import { expect, mock, test } from 'claude-code/testing'

test('/revu opens the branch PR in a herdr split and relays revu messages', async ($, on) => {
  mock.env(on, { HOME: '/home/me', HERDR_PANE_ID: 'w:p1' })
  on('session.id', () => ({ value: 'S1' }))
  on('session.cwd', () => ({ value: '/repo' }))
  const written: string[] = []
  on('fs.write', (_$, e) => {
    written.push(e.path)
    return { value: undefined }
  })
  const runs: string[][] = []
  on('process.run', (_$, e) => {
    runs.push([...e.argv])
    const out = (stdout: string) => ({ value: { exitCode: 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } })
    if (e.argv[0] === 'gh') return out('42\n')
    if (e.argv[2] === 'split') return out('{"result":{"pane":{"pane_id":"w:p2"}}}')
    return out('')
  })
  on('process.spawn', async function* () {
    yield { stream: 'stdout' as const, text: '{"text":"why this?"}\n{"te' }
    yield { stream: 'stdout' as const, text: 'xt":"and that"}\n' }
    return { value: { code: 0, signal: null } }
  })
  const prompts: string[] = []
  on('prompt.submit', (_$, e) => {
    prompts.push(e.text)
    return e.text === 'and that' ? { drop: 'blocked' } : { text: e.text }
  })
  const toasts: string[] = []
  on('ui.toast', (_$, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })

  on('session.start', (_$, e) => e)
  on('command.register', (_$, e) => ({ value: { command: e.name } }))
  const clock = mock.clock(on)
  await $.session.start({ cwd: '/repo' } as Parameters<typeof $.session.start>[0])
  await clock.settle()
  expect(prompts).toEqual(['why this?', 'and that'])
  expect(toasts).toEqual(['revu: message not sent: blocked'])

  const { text } = await $.command.run({ command: 'revu', args: '' } as Parameters<typeof $.command.run>[0])
  expect(text).toBe('revu opened in a herdr split.')
  const inbox = '/home/me/.cache/revu/claude/S1.jsonl'
  expect(written).toEqual([inbox])
  expect(runs[1]).toEqual(['sh', '-c', 'command -v "$1"', 'sh', 'revu'])
  expect(runs[2]?.slice(0, 3)).toEqual(['herdr', 'pane', 'split'])
  expect(runs[2]).toContain(`REVU_CLAUDE_INBOX=${inbox}`)
  expect(runs[3]?.slice(0, 4)).toEqual(['herdr', 'pane', 'run', 'w:p2'])
  expect(runs[3]?.[4]).toContain('revu --pr 42')
})

test('/revu says so and opens nothing when revu is not installed', async ($, on) => {
  mock.env(on, { HOME: '/home/me', HERDR_PANE_ID: 'w:p1' })
  on('session.cwd', () => ({ value: '/repo' }))
  const runs: string[][] = []
  on('process.run', (_$, e) => {
    runs.push([...e.argv])
    return { value: { exitCode: e.argv[0] === 'sh' ? 1 : 0, stdout: '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('fs.write', () => ({ value: undefined }))

  const { text } = await $.command.run({ command: 'revu', args: '7' } as Parameters<typeof $.command.run>[0])
  expect(text).toContain('`revu` is not installed')
  expect(runs.map(r => r[0])).toEqual(['sh'])
})
