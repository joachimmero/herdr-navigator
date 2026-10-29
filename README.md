# herdr-navigator

A [herdr](https://herdr.dev) plugin that opens a pane for browsing and reading a project beside
the chat.

- `herdr-navigator src/app.rs:40-52` shows a file in the workspace's pane with those lines marked,
  so you or a coding agent can point at a place you can read without leaving the terminal.
- The pane follows the file. When it changes on disk, the pane rereads it and keeps your place.
  While the file is gone it keeps the last content and says the file is missing.
- The project's files sit in a tree beside the file, which shows each file as you move onto it
  and keeps up as files come and go.
- Markdown shows rendered: headings, emphasis, lists and task lists, quotes, code blocks, rules,
  links (with their target shown), footnotes, and tables, which shrink their columns and wrap
  cells to fit the pane. `m` flips it to its source and back, for every file the pane opens.
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
description = "toggle navigator"
```

The pane opens as a split beside the focused pane, in that pane's directory.

## Pointing at a file from the command line

```sh
herdr-navigator <file>[:line[-line]]
```

- Run it from a herdr pane. A relative path is taken from the current directory.
- It shows the file in the workspace's herdr-navigator pane, or opens one beside the calling pane
  without taking the focus, then returns at once.
- With lines, the cursor goes to the first of them and they're marked in the margin until the
  cursor moves off them.
- While you're writing a comment the pane stays put, and the title bar shows where it was pointed;
  `⏎` goes there once the comment is saved or cancelled.
- Exit codes: `0` shown, `1` the file doesn't exist, the command ran outside herdr, or herdr
  failed, `2` bad arguments.

## Keys

In a file:

| Key | Does |
| --- | --- |
| `j` `k` / `↓` `↑`, click | Move the cursor |
| mouse wheel | Scroll, taking the cursor along |
| drag | Copy the text dragged over |
| `Ctrl+d` `Ctrl+u` | Move half a page down, up |
| `space` / `PgDn`, `PgUp` | Move a page down, up |
| `g` `G` / `Home` `End` | Go to the top, the bottom |
| `v` | Start selecting lines at the cursor, or stop |
| `c` | Comment on the selection, or the cursor's line |
| `e` `d` | Edit, delete the comment at the cursor |
| `n` `N` | Go to the next, previous place the kept find is at, or else comment in any file |
| `Ctrl+o` `Ctrl+i` | Go back to where a jump left from, forward again |
| `/` | Find text in the file |
| `Ctrl+f` | Search the project |
| `m` | Show markdown as its source, or rendered again |
| `l` | List every comment |
| `s` | Send every comment to an agent |
| `y` | Copy every comment |
| `Esc` | Drop the find, or the selection, or go to the tree |
| `Tab` | Go to the tree |
| `q` | Close the pane |

In the tree:

| Key | Does |
| --- | --- |
| `j` `k` / `↓` `↑`, mouse wheel | Move, showing each file as you land on it |
| `Ctrl+d` `Ctrl+u` | Move half a page down, up |
| `g` `G` / `Home` `End` | Go to the first, the last entry |
| `⏎` `l` `→`, click | Open or close a folder, or go into the file |
| `h` `←` | Close the folder, or go to the one holding the entry |
| `i` | Show or hide what git ignores |
| `Tab` | Go to the file |
| `/` | Find text in the file |
| `Ctrl+f` | Search the project |
| `m` | Show markdown as its source, or rendered again |
| `Ctrl+o` `Ctrl+i` | Go back to where a jump left from, forward again |
| `r` | Look for files again |
| `q` | Close the pane |

The cursor and the selection are marked in the margin, and the title bar shows the source lines
they cover, so in rendered markdown you can see which lines of the file a paragraph came from.

Dragging over the file copies the text as it's shown, without line numbers or bullets, a line
break at the end of each row on screen. Dragging past the top or bottom scrolls. The copied text
stays marked until the next key or click.

Pointing from the command line, `n` `N` between comments, opening a search result, and `⏎` in the
comment list are jumps: `Ctrl+o` goes back to where each one left from, like vim's jumplist.
`Ctrl+i` needs a terminal that tells it apart from `Tab`; inside tmux it arrives as `Tab`.

## Find in file

`/` opens a band under the file and moves the cursor to the text as you type, marking each
place it's found. Case matters only when what you type holds a capital. `↓` goes to the next
place and `↑` to the previous.

`⏎` closes the band and keeps the find, or cancels when nothing is found: its places stay marked, in every file you open, and the
title bar shows it with which place the cursor is on. While it's kept, `n` `N` go to the next,
previous place, wrapping around, instead of the next comment. `Esc` in the file drops it.

`Esc` in the band cancels, putting the cursor back where it was and keeping any earlier find.
Comment cards aren't searched, and text broken across a wrapped line isn't found.

## Search

`Ctrl+f` searches the project's file paths and their contents as you type, with
[fff](https://github.com/dmtrKovalenko/fff.nvim)'s engine. The list shows the files that match,
and `Tab` or a click on the chips switches it to the lines that match, grouped by file. The
picked result shows in a preview below the list, a matching line in the middle of it.

| Key | Does |
| --- | --- |
| `↓` `↑` / `Ctrl+n` `Ctrl+p`, mouse wheel | Pick a result |
| `⏎`, clicking the picked result | Open it, at the line for a matching line |
| `Tab` | Switch between files and lines |
| `PgDn` `PgUp`, mouse wheel over the preview | Scroll the preview |
| `Esc` | Close the search |

The first search in a pane indexes the project, which keeps up as files change. Files you open
from search rank higher in later ones.

## Comments

A comment sits in a card under the lines it's on, and stays for the session. In the comment box,
`⏎` saves, `Shift+⏎`, `Alt+⏎` or `Ctrl+j` starts a new line, and `Esc` cancels; saving an empty
box cancels too. In the list, `j` `k` move, `⏎` jumps to the comment, `d` deletes it, and `Esc`
or `l` closes the list.

When the file changes, each comment follows the lines it quoted to wherever they now sit, the
nearest place when they appear more than once. A comment whose lines are gone is marked outdated
and stays where they last were.

`s` pastes every comment into an agent's input in this workspace, without submitting it, and
focuses that agent so you can add to it. With several agents a picker opens on the one you sent
to last; `j` `k` or `1`–`9` choose and `⏎` sends. An agent waiting at a prompt refuses the
paste. `y` copies the same text instead. Either way the comments are let go of once delivered,
and each is written as its location, the lines it's on, then what it says:

```
src/doc.rs:12-14
fn reflow(&mut self) {
    self.rows.clear();
}
Why clear here rather than on resize?
```

A comment whose lines are gone is sent with ` (outdated)` after its location and the lines it was
written on.

## The tree

It lists the files under the git root of the pane's directory, or under the directory itself
outside git, folders first. What git ignores is hidden until you press `i`, then shown dim, and an
ignored folder is only read when you open it. Below 70 columns only the tree or the file shows, and
`Tab` swaps them.

## Limits

- Links aren't followed; the tree is the way between files.
- Code blocks aren't syntax-highlighted.
- Images show as their alt text, and raw HTML as dim source. HTML comments are hidden.
