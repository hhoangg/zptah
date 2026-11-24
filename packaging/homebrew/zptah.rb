# Template for Casks/zptah.rb in the tap hhoangg/homebrew-zptah.
#
# The release workflow only bumps the `version` and `sha256` lines of an existing
# cask (scripts/update-homebrew-cask.sh); it never creates the file. Copy this
# file to Casks/zptah.rb in the tap once, by hand. The placeholder version 0.0.0
# and the all-zero sha256 are replaced by the first release.
cask "zptah" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/hhoangg/zptah/releases/download/native-v#{version}/zptah_#{version}_aarch64.zip"
  name "zptah"
  desc "One UI for every agent harness"
  homepage "https://zptah.app"

  livecheck do
    url "https://github.com/hhoangg/zptah/releases"
    regex(/^native-v(\d+(?:\.\d+)+)$/i)
    strategy :github_releases
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on :macos

  app "zptah.app"
end
