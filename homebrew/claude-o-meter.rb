# Homebrew Cask template — copy this to your tap repo (e.g.
# `homebrew-tap/Casks/claude-o-meter.rb`) and bump `version` + `sha256`
# on each release. Users then install with:
#
#   brew install --cask <your-github-user>/tap/claude-o-meter
#
# The sha256 below is a placeholder; replace with the value from the
# release's `Claude-O-Meter.zip.sha256` asset.

cask "claude-o-meter" do
  version "0.1.0"
  sha256 "REPLACE_ME_WITH_RELEASE_SHA256"

  url "https://github.com/REPLACE_ME_WITH_YOUR_GH_USER/Claude-O-Meter/releases/download/v#{version}/Claude-O-Meter.zip"
  name "Claude-O-Meter"
  desc "Menu-bar app for Claude Code session and weekly quota monitoring"
  homepage "https://github.com/REPLACE_ME_WITH_YOUR_GH_USER/Claude-O-Meter"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :ventura"

  app "Claude-O-Meter.app"

  postflight do
    # Clear Gatekeeper quarantine — the binary is ad-hoc signed (not
    # notarized), so without this users get a "developer cannot be
    # verified" warning on first launch.
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Claude-O-Meter.app"],
                   sudo: false
  end

  zap trash: [
    "~/Library/Application Support/com.cynkra.claude-o-meter",
  ]
end
