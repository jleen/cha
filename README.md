# Cha 茶

Cha 茶 is a pattern-matching and anagramming word tool intended to assist
in solving cryptic crosswords, acrostics, and other such word puzzles.  Cha
understands a pattern language based on the classic TEA Crossword Helper.
(Cha, however, has no relation to TEA other than as a source of inspiration.)

Cha is provided as a cross-platform GUI application that will run on Mac OS,
Windows, and Linux; as well as a CLI tool that will run on pretty much anything
that Rust can target.  In addition, the core pattern matcher is provided as a
standalone crate.

## AI Statement

As you‘ll notice from the commit history, this code is mostly AI-generated.
This is my (the author‘s) first nontrivial AI-assisted software project, so I
make no claims about code quality or even whether I‘m doing any of this the
right way. On the other hand, I‘ve been using this codebase for my own
puzzle-solving activities and it‘s been useful to me, so I offer it in the hope
that it might be useful to you.

The comments in the source code, and the technical documentation in the `docs`
directory, are mostly written by and for the LLM. This README and the in-app
help screen are written by and for humans.

## Word list

A word list is provided, based on the [12dicts](https://wyrdplay.org/12dicts.html) lists. You can also provide your own word list as a text file, one word per line; case and accents are normalized for matching and preserved for display.

The GUI expects user-provided word list files to be located in a designated directory
(see below).  If you’re building your own binaries,
you can also just replace `words.txt` with your preferred list; it will be compiled into the GUI
application and bundled with it, to be used in addition to any
user-provided word lists.

## Implementation notes and agent memory

Design notes, invariants, build process, and release procedures for each module live in
[docs/](docs/). Those documents are written by Claude, primarily for its own use,
but mentioned here since they might be a useful reference for human collaborators as well.
See
[docs/README.md](docs/README.md) for more detail.

## GUI usage

Just install and run the application. You can immediately start doing
word searches in the default dictionary. If you want to bring your own dictionary, File -> Open Dictionary Folder will open a folder on your desktop where you can drop a custom dictionary
file, one word per line.

## CLI usage

```
cha <pattern> [-d] [-w wordlist] [-b bench_count]
cha -i        [-d] [-w wordlist]
```

By default, `cha` loads its word list from `./words.txt`. Specify a different
word list with `-w`.

Either specify a pattern on the command line, or pass `-i` / `--interactive` to
enter an interactive loop which will repeatedly prompt for a pattern and return
results.  Enter `^D` on an empty line to exit.

Default output is just a list of matching words. With `-d`, you‘ll
also see the added or dropped letters from an anagram pool.

## Pattern Syntax

See the [pattern syntax overview](cha-gui/help/pattern-syntax.md),
which also appears as the in-app help text.

## Building

### CLI

```
cargo build --release
```

The CLI is built at `target/release/cha`.

### GUI

The GUI uses [Tauri](https://tauri.app/). Install the Tauri build tool with

```
cargo install tauri-cli
```

On Linux you’ll need to install Tauri’s dependencies. On a Debian-based distro,
something like
`apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev build-essential libssl-dev` should do the trick.

Once Tauri and its dependencies are installed, you can build from the project
root:

```
cargo tauri dev      # run a debug build
cargo tauri build    # compile and package a release build
```

## Benchmarking

The CLI has a “benchmarking” mode: if you run `cha <pattern> -b<N>`, it will load the dictionary once, and then run the pattern search *N* times in succession,
for performance profiling purposes (the goal being to isolate
the load-time overhead from the search performance).
