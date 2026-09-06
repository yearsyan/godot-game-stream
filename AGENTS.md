# AGENTS.md

This is the standalone LGPL-2.1-or-later Godot Game Stream addon and mirctl client.

- Write all documentation, comments, command-line messages and commit messages in English.
- Native builds use `python3 scripts/build.py` (invokes `pug extension build`).
  Do not compile a custom Godot engine for this addon.
- Use `cargo test --locked --release`, Clippy and rustfmt for native checks.
- Client builds use `python3 clients/mirctl/scripts/build.py`; run CTest afterward.
- Test byte-level protocol changes against the shared `protocol/fixtures/` in both languages.
- Do not add GPL/nonfree FFmpeg components to the default LGPL distribution.
- Never commit native SDKs, generated binaries, local machine paths or caches.
- Preserve the loopback default and explicit opt-in to starting a stream.
- Keep developer build details out of the host node's normal UI.
- Update docs and matching source/license release materials when changing dependencies.
