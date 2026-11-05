import type { EngineInterface, Register } from 'claude-code'

type $ = EngineInterface

/** POSIX single quoting; fish reads `'\''` the same way. */
const quote = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`

/** revu appends one `{"text": ...}` JSON line per message to this file. */
async function inboxPath($: $): Promise<string> {
  const cache = (await $.env.get('XDG_CACHE_HOME')) ?? `${await $.env.get('HOME')}/.cache`
  return `${cache}/revu/claude/${await $.session.id()}.jsonl`
}

/** Submits each message revu writes to the inbox as the person's own prompt. */
async function watch($: $, inbox: string) {
  // tail -F waits for the file, which /revu creates.
  let buf = ''
  for await (const { stream, text } of $.process.spawn({ argv: ['tail', '-n0', '-F', inbox] })) {
    if (stream !== 'stdout') continue
    buf += text
    const lines = buf.split('\n')
    buf = lines.pop() ?? ''
    for (const line of lines.filter(Boolean)) {
      try {
        const { text: msg } = JSON.parse(line) as { text: string }
        // Framed as the plugin's: the message quotes diff and comments by others.
        $.prompt
          .submit({ text: msg })
          .then(r => r.drop && $.ui.toast(`revu: message not sent: ${r.drop}`))
          .catch(err => $.ui.toast(`revu: message not sent: ${(err as Error).message}`))
      } catch {
        $.ui.log(`revu: unreadable inbox line: ${line.slice(0, 80)}`, { to: 'debug' })
      }
    }
  }
}

/** Opens `shell` beside Claude: a herdr or tmux split, else tells the person how. */
async function openBeside($: $, shell: string, cwd: string, inbox: string): Promise<string> {
  const env = `REVU_CLAUDE_INBOX=${inbox}`
  if (await $.env.get('HERDR_PANE_ID')) {
    const split = await $.process.run(
      ['herdr', 'pane', 'split', '--current', '--direction', 'right', '--cwd', cwd, '--env', env, '--focus'],
    )
    if (split.exitCode !== 0) throw new Error(`herdr pane split: ${split.stderr.trim()}`)
    const pane = (JSON.parse(split.stdout) as { result: { pane: { pane_id: string } } }).result.pane.pane_id
    const run = await $.process.run(['herdr', 'pane', 'run', pane, `exec sh -c ${quote(shell)}`])
    if (run.exitCode !== 0) throw new Error(`herdr pane run: ${run.stderr.trim()}`)
    return 'revu opened in a herdr split.'
  }
  if (await $.env.get('TMUX')) {
    const run = await $.process.run(['tmux', 'split-window', '-h', '-c', cwd, '-e', env, 'sh', '-c', shell])
    if (run.exitCode !== 0) throw new Error(`tmux split-window: ${run.stderr.trim()}`)
    return 'revu opened in a tmux split.'
  }
  const line = `cd ${quote(cwd)} && REVU_CLAUDE_INBOX=${quote(inbox)} sh -c ${quote(shell)}`
  const { isCopied } = await $.ui.copy({ text: line })
  return `Not in herdr or tmux: run this in another terminal${isCopied ? ' (copied)' : ''}:\n${line}`
}

export const register: Register = (on, options) => {
  const command = String(options.command ?? 'revu')
  // Fixed at start: a /clear changes the session id, not the watched file.
  let inbox = ''

  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await $.command.register({
      name: 'revu',
      description: "Review this branch's PR in revu, beside Claude (`a` in the diff sends to Claude)",
      argumentHint: '[pr number]',
    })
    inbox = await inboxPath($)
    watch($, inbox).catch(err => $.ui.log(`revu: inbox watch stopped: ${(err as Error).message}`, { to: 'debug' }))
    return started
  })

  on('command.run', { command: 'revu' }, async ($, e) => {
    const cwd = await $.session.cwd()
    let pr = e.args.trim()
    if (!pr) {
      const view = await $.process.run(['gh', 'pr', 'view', '--json', 'number', '-q', '.number'], { cwd })
      if (view.exitCode !== 0) return { text: `No pull request for this branch: ${view.stderr.trim()}` }
      pr = view.stdout.trim()
    }
    if (!/^\d+$/.test(pr)) return { text: `Not a PR number: ${pr}` }

    const bin = command.trim().split(/\s+/)[0] ?? ''
    // Resolved here: a herdr split starts from herdr's PATH, not this one (e.g. a `nix shell`).
    const found = await $.process.run(['sh', '-c', 'command -v "$1"', 'sh', bin])
    if (found.exitCode !== 0) {
      return { text: `revu not found: \`${bin}\` is not installed. Install revu (https://github.com/alexpill/revu#install) or set the plugin's command option.` }
    }

    await $.fs.write(inbox, '')
    // A failed start keeps the pane open on its error.
    const exe = `${quote(found.stdout.trim())}${command.trim().slice(bin.length)}`
    const shell = `${exe} --pr ${pr} || { echo; echo "revu failed, press Enter to close"; read _; }`
    try {
      return { text: await openBeside($, shell, cwd, inbox) }
    } catch (err) {
      return { text: `Could not open revu: ${(err as Error).message}` }
    }
  })
}
