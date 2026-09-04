# Releasing Viode

Cutting a release is a tag push; CI does the rest. This checklist is
the whole process — follow it top to bottom every time.

## Every release

1. Start from a clean, pushed master with the CI badge green.
2. Confirm the workspace version in `Cargo.toml` matches the tag you
   are about to cut. Bump it in its own commit if it does not.
3. Tag and push:

   ```bash
   git tag -a v0.1.0 -m "Viode v0.1.0"
   git push origin v0.1.0
   ```

4. The Release workflow builds Linux x86_64 and macOS arm64 binaries,
   runs the full test suite on both as a final gate, and attaches
   `viode-<tag>-<platform>.tar.gz` archives with SHA-256 checksums to
   the GitHub Release. Watch it: `gh run watch`.
5. Verify the release page shows both archives and both checksums.
   `https://github.com/edimoldovan/viode/releases/latest` is the URL
   everything else points at — nothing needs a version bump.

## Once the package managers exist (distribution phase)

6. AUR: bump `pkgver` in the PKGBUILD, update checksums, push to the
   AUR remote.
7. Homebrew: bump the version and sha256 in the formula in
   `edimoldovan/homebrew-viode`.

## Switching to official (license-checking) builds

The binaries this workflow ships today are evaluation builds of the
public repository. Official builds wrap `viode_cli::run()` in the
private `viode-license` crate. To switch the release artifacts over:

1. Ed pushes `~/dev/linux/viode-license` to a private GitHub remote.
2. A read-only deploy key (or fine-grained PAT) for that repository
   goes into this repository's Actions secrets.
3. The Release workflow gains a checkout of the private crate next to
   this one and builds its `viode` binary instead of `-p viode-cli`.

Until then, every install is ungated on purpose — Viode is free for
now, and the announcement channel begins mattering when key-checking
versions ship (see CLAUDE.md, "License enforcement").
