# herdr-navigator

A [herdr](https://herdr.dev) plugin that opens a pane for reading markdown files beside the chat.

- `herdr-navigator PLAN.md` shows a file in the workspace's pane, so you or a coding agent can put a
  doc where you can read it without leaving the terminal.
- The pane follows the file. When it changes on disk, the pane rereads it and keeps your place.
  While the file is gone it keeps the last content and says the file is missing.
- With no file to show, the pane lists the project's markdown files to pick from.
- Headings, emphasis, lists and task lists, quotes, code blocks, rules, links (with their target
  shown), footnotes, and tables, which shrink their columns and wrap cells to fit the pane.
- Any other file shows as its source, syntax-highlighted in the terminal's own colors, with line
  numbers. A binary file shows a notice instead.

## Requirements

- macOS, herdr 0.9.3 or later.
- `cargo`, to build the plugin. Install it with [rustup](https://rustup.rs); the first build
  fetches the Rust version pinned in `rust-toolchain.toml`.

## Install (local)

```sh
scripts/link.sh
```

That builds the release binary with `cargo`, links `bin/herdr-navigator` to it, puts
`herdr-navigator` on `PATH` through `~/.local/bin`, and runs `herdr plugin link`. Then bind the
toggle to a key in herdr's `config.toml`:

```toml
[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "joachimmero.herdr-navigator.toggle"
description = "toggle markdown reader"
```

The pane opens as a split beside the focused pane, in that pane's directory.

## Showing a file from the command line

```sh
herdr-navigator <file>
```

- Run it from a herdr pane. A relative path is taken from the current directory.
- It shows the file in the workspace's herdr-navigator pane, or opens one beside the calling pane
  without taking the focus, then returns at once.
- Exit codes: `0` shown, `1` the file doesn't exist, the command ran outside herdr, or herdr
  failed, `2` bad arguments.

## Keys

In a file:

| Key | Does |
| --- | --- |
| `j` `k` / `↓` `↑`, mouse wheel | Scroll |
| `Ctrl+d` `Ctrl+u` | Scroll half a page down, up |
| `space` / `PgDn`, `PgUp` | Scroll a page down, up |
| `g` `G` / `Home` `End` | Go to the top, the bottom |
| `Esc` `o` | Go to the file list |
| `q` | Close the pane |

In the file list:

| Key | Does |
| --- | --- |
| `j` `k` / `↓` `↑`, mouse wheel | Move |
| `Ctrl+d` `Ctrl+u` | Move half a page down, up |
| `g` `G` / `Home` `End` | Go to the first, the last file |
| `⏎` | Open the file |
| `/` | Filter by path (`⏎` keeps it, `Esc` clears it, `⌫` on an empty filter leaves it) |
| `r` | Look for files again |
| `q` | Close the pane |

## The file list

It lists the `.md` and `.markdown` files under the git root of the pane's directory, or under the
directory itself outside git, skipping what git ignores. It looks again each time you go back to
it, keeping your filter and the file you had selected.

## Limits

- Links aren't followed; the picker is the way between docs.
- Code blocks aren't syntax-highlighted.
- Images show as their alt text, and raw HTML as dim source. HTML comments are hidden.
