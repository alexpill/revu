# prt

Revue de pull requests GitHub dans le terminal : tableau de bord multi-repos,
page de PR façon GitHub, diff avec commentaires inline, review en attente,
statut « viewed » synchronisé, et navigation dans le code (fichiers, go to
definition, références) sans quitter l'outil.

Toutes les données passent par le [CLI `gh`](https://cli.github.com/) : si
`gh auth status` est bon, `prt` marche.

## Installation

```sh
cargo install --path .
prt
```

Prérequis : `gh` authentifié, `git`, et les serveurs LSP voulus dans le `PATH`.

## Configuration

`$XDG_CONFIG_HOME/prt/config.toml` (par défaut `~/.config/prt/config.toml`) :

```toml
theme = "Catppuccin Mocha"     # optionnel, n'importe quel thème bat

[[repos]]
name = "owner/repo"
path = "~/code/repo"           # optionnel : clone local

[[repos]]
name = "owner/autre-repo"

[lsp]                          # optionnel : extension -> commande
ts = ["typescript-language-server", "--stdio"]
nix = ["nixd"]
```

Sans `[[repos]]`, `prt` prend le repo GitHub du dossier courant.

Le `path` d'un repo active :
- la lecture des fichiers en local (`git show`, pas de limite de taille de l'API) ;
- le LSP : au premier `gd`, `prt` récupère `refs/pull/N/head` et crée un
  worktree détaché dans `~/.cache/prt/worktrees/`. Ton checkout n'est jamais modifié.

Serveurs par défaut : `rust-analyzer`, `typescript-language-server`,
`pyright-langserver`, `gopls`, `nil`, `lua-language-server`, `clangd`, `zls`, `ruby-lsp`.

## Utilisation

`?` affiche les raccourcis de l'écran courant.

**Accueil / repos**

| Touche | Action |
|---|---|
| `1`-`9`, `H`/`L`, `Tab` | onglets (1 = accueil) |
| `p` / `i` | PR / issues du repo |
| `f` | filtre PR : toutes, review demandée, assignées, les miennes, drafts |
| `/` | filtre les PR, ou recherche d'issues en syntaxe GitHub (`is:closed label:bug`) |
| `Enter`, `o` | ouvrir, ouvrir dans le navigateur |
| `Ctrl-r` | rafraîchir |

**PR**

| Touche | Action |
|---|---|
| `1` `2` `3`… `H`/`L` | conversation, fichiers, buffers ouverts |
| `Tab` | fichiers : liste ↔ diff |
| `[` `]` / `{` `}` | fichier / hunk précédent, suivant |
| `Space` | marquer vu (immédiat, synchro GitHub en fond) et passer au suivant non vu |
| `j/k h/l w/b 0/$ gg/G Ctrl-d/u` | déplacements |
| `v` puis `c` | commenter une ligne ou une plage (dans la review en attente) |
| `r` / `R` | sur un thread : répondre / résoudre |
| `z` | déplier un fichier généré (lockfiles repliés par défaut) |
| `gd` `gr` `K` | définition, références, hover (LSP) |
| `Ctrl-p` | ouvrir n'importe quel fichier de la PR |
| `/` `n` `N` | rechercher dans un buffer |
| `Ctrl-o` | revenir en arrière |
| `x` | fermer le buffer |
| `e` | ouvrir l'endroit courant dans `$EDITOR` |
| `S` | soumettre la review : commenter, approuver, demander des changements, abandonner |
| `C` | commentaire général sur la PR |

Les commentaires s'écrivent dans `$VISUAL`/`$EDITOR` ; `:cq` annule.

## Architecture

```
src/
├── main.rs          démarrage du terminal
├── config.rs        config.toml, repo du dossier courant
├── gh/              accès GitHub via `gh api`
│   ├── mod.rs       requêtes GraphQL/REST (listes, PR, fichiers, issues)
│   ├── model.rs     types des réponses
│   └── mutation.rs  écritures, exécutées dans l'ordre sur un thread dédié
├── source.rs        contenu des fichiers : clone local ou API, worktrees
├── diff.rs          patch → lignes numérotées (ancres LEFT/RIGHT)
├── highlight.rs     coloration (syntect + syntaxes et thèmes de bat)
├── markdown.rs      rendu Markdown minimal, nettoie le HTML des bots
├── lsp.rs           client LSP minimal (JSON-RPC sur stdio)
└── app/
    ├── mod.rs       boucle d'événements, pile d'écrans, jobs en arrière-plan
    ├── main_screen.rs  accueil + onglets repo
    ├── issue.rs     page d'une issue
    ├── picker.rs    sélecteur flou (fichiers, résultats LSP)
    ├── ui.rs        helpers de rendu partagés
    └── pr/          page d'une PR
        ├── conversation.rs  description, timeline, sidebar
        ├── files.rs         liste de fichiers + diff + threads
        └── code.rs          buffers de code en lecture seule
```

Tout ce qui fait des I/O (`gh`, `git`, LSP) tourne sur des threads qui
répondent par un unique canal de `Msg` : l'interface ne bloque jamais. Les
écritures (vu, commentaires, review) sont appliquées tout de suite en local
puis envoyées dans l'ordre par un worker unique.

Pour ajouter un écran : une variante de `Screen` dans `app/mod.rs`, avec
`on_key`, `on_msg`, `render` et `HELP`.

## Limites connues

- 30 PR ouvertes max par repo (les plus récemment mises à jour).
- « Review demandée » ne tient pas compte des demandes faites à une équipe.
- Diff unifié seulement ; pas de défilement horizontal des lignes longues.
- Fichiers sans patch dans l'API (binaires, très gros) : pas de diff affiché.
- Pas encore d'actions IA ni de plugins : les points d'extension naturels sont
  `External` (commande externe avec le terminal) et `Outcome` (actions des onglets).
