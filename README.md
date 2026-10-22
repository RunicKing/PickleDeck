# PickleDeck

**PickleDeck** is a community fork of [OpenDeck](https://github.com/nekename/OpenDeck) — desktop software for stream controller devices such as the **Elgato Stream Deck**, built as an open source alternative controller hub.

## What this fork is

- Maintained as a **remix for one user first**: this fork exists mainly for its owner, but **anyone is welcome to use PickleDeck over the upstream OpenDeck if they prefer it**.
- Goals: keep full plugin-protocol compatibility, then extend (see the goals below):
  profile import *from the Elgato software*, press-gesture logic (tap / double-tap / hold),
  an internal virtual deck window, and custom theming.
- Upstream fixes may be pulled in periodically; this fork is not a replacement for
  upstream's development.

## AI disclaimer

**Any and all changes made in this repository are produced by an AI model, nicknamed
"Doctor Pickle",** working under the direction of this fork's owner
([RunicKing](https://github.com/RunicKing)). No human writes code here directly.

## Compatibility by design

The fork deliberately keeps upstream's internal identity and plugin namespaces
(`opendeck` config directory, built-in `opendeck.*` action UUIDs, OpenDeck property
inspector URL markers, deep-link schemes). Existing plugins written for OpenDeck — and
most Stream Deck SDK plugins — work unchanged. Displayed names and branding are the part
that changes.

## Building

Requires **Rust** (MSVC), **Deno**, and on Windows the **VS Build Tools** C++ workload:

```sh
deno install           # fetch frontend dependencies
deno task build        # build SvelteKit frontend into ../build
cargo build --manifest-path src-tauri/Cargo.toml
```

Run the debug binary from `src-tauri/target/debug/`. For a dev UI loop: `deno task tauri dev`.

## Attribution & license

- Upstream project: **OpenDeck** by Aman Khanna (nekename),
  [github.com/nekename/OpenDeck](https://github.com/nekename/OpenDeck). Thank you for the
  foundation — this fork would not exist without it.
- Plug-in ecosystem reference: the
  [OpenAction catalogue](https://github.com/OpenActionAPI/plugins).
- License: **GPL-3.0-or-later** (same as upstream). PickleDeck is an independent project
  and is not affiliated with, sponsored by, or endorsed by Elgato. "Elgato Stream Deck" is
  used only descriptively, to identify the class of device supported.
