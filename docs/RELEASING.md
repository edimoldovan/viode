# Releasing Viode

The rule (CLAUDE.md, "Release rules"): a release goes public only when
a user can install Viode from it, every direct download is complete in
itself, and package managers are the only dependency exception. The
Packages workflow proves the installers continuously — every push
builds the .deb, .rpm, Arch package, Homebrew formula, and the Mac
bundle, and installs each one in a clean environment ending with
`viode doctor`. Release day is therefore short: tag, verify the draft,
push the package-manager entries, publish.

## One-time setup (Ed)

- Apple signing: the Developer ID certificate and notarization
  credentials go into this repository's Actions secrets. The mac-app
  job signs and notarizes when they are present; until then bundles
  are ad-hoc signed and good for testing only.
- AUR: an AUR account with an SSH key, and the `viode` package name
  claimed (first push claims it).
- Homebrew: create the tap repository `edimoldovan/homebrew-viode`.
- Decide whether release binaries are the official license-checking
  builds (private viode-license crate needs a remote and a read-only
  deploy key in the secrets) or public evaluation builds. Viode is
  free for now, so evaluation builds are the current answer.

## Release day

1. Start from a clean, pushed master with CI and Packages both green,
   and the workspace version in `Cargo.toml` matching the tag.
2. Tag and push — this builds everything in full:

   ```bash
   git tag -a v0.1.0 -m "Viode v0.1.0"
   git push origin v0.1.0
   ```

   The Packages workflow runs with FULL_ENGINE (vidstab ffmpeg, models
   inside the Mac bundle) and stages the .deb, .rpm, and .dmg on a
   DRAFT GitHub release. Drafts are invisible to the public.
3. Verify the draft like a user, not a packager: on a Mac without
   Homebrew, download the .dmg, drag Viode to Applications, open it,
   and check the engine checkup is all green; on Linux, install the
   .deb or .rpm in a fresh container or VM and do the same.
4. AUR: in `packaging/aur/`, set the tag tarball's sha256 in the
   PKGBUILD, regenerate `.SRCINFO` (`makepkg --printsrcinfo >
   .SRCINFO`), and push both to `ssh://aur@aur.archlinux.org/viode.git`.
5. Homebrew: copy `packaging/homebrew/viode.rb` into the tap with the
   tag URL and the same sha256, and push the tap.
6. Prove the package managers end to end: `yay -S viode` on Arch and
   `brew install edimoldovan/viode/viode` on a Mac.
7. Publish the draft release. `/releases/latest` is now the stable
   download URL.
8. Same working session (standing rule): point the marketing page's
   download links at the release and check the manual's install
   chapter still tells the truth.
