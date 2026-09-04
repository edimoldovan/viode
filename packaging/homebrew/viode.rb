# The Homebrew install story: `brew install edimoldovan/viode/viode`
# pulls the engine (GStreamer with Editing Services, ffmpeg) as
# declared dependencies — the user reads no dependency list. This file
# is the source of truth; at release time it is copied into the tap
# repository (edimoldovan/homebrew-viode) with the url and sha256
# pointing at the tagged tarball.
#
# Known brew platform gaps, surfaced by `viode doctor` rather than
# solved here: brew's gstreamer ships without soundtouch (speed changes
# need it) and brew's core ffmpeg dropped libvidstab (stabilization
# needs it; the homebrew-ffmpeg community tap has a --with-libvidstab
# build). The self-contained Mac app carries a complete engine and has
# neither gap.
class Viode < Formula
  desc "AI-native video editor: desktop app, terminal UI, CLI, one engine"
  homepage "https://github.com/edimoldovan/viode"
  url "https://github.com/edimoldovan/viode/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "FILLED-IN-AT-RELEASE-TIME"
  license "PolyForm-Free-Trial-1.0.0"

  depends_on "pkgconf" => :build
  depends_on "rust" => :build
  depends_on "ffmpeg"
  depends_on "gstreamer"

  def install
    system "cargo", "install", "--locked", "--root", prefix, "--path", "crates/viode-cli"
  end

  test do
    assert_match "viode", shell_output("#{bin}/viode --version")
    system bin/"viode", "doctor"
  end
end
