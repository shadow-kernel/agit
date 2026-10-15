# Homebrew formula for agit. Template: the release workflow renders a copy
# with the real version and SHA-256 values and attaches it to every GitHub
# release as `agit.rb`. To publish, copy that rendered file into a tap
# (for example github.com/agit-vcs/homebrew-tap as Formula/agit.rb) so users
# can run:
#
#   brew install agit-vcs/tap/agit
#
# Or install straight from the release asset:
#
#   brew install --formula https://github.com/agit-vcs/agit/releases/download/v0.1.0/agit.rb
#
# Placeholders SHA256_<TARGET> are replaced by .github/workflows/release.yml.
class Agit < Formula
  desc "Version control and collaboration for AI agents, with git's workflow"
  homepage "https://github.com/agit-vcs/agit"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/agit-vcs/agit/releases/download/v#{version}/agit-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "SHA256_AARCH64_APPLE_DARWIN"
    end
    on_intel do
      url "https://github.com/agit-vcs/agit/releases/download/v#{version}/agit-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "SHA256_X86_64_APPLE_DARWIN"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/agit-vcs/agit/releases/download/v#{version}/agit-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256_AARCH64_UNKNOWN_LINUX_GNU"
    end
    on_intel do
      url "https://github.com/agit-vcs/agit/releases/download/v#{version}/agit-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "SHA256_X86_64_UNKNOWN_LINUX_GNU"
    end
  end

  # `agit git import|export` shell out to git.
  depends_on "git" => :recommended

  def install
    # Tarballs unpack to agit-<version>-<target>/; Homebrew strips that directory.
    bin.install "agit", "agit-server"
    (var/"agit").mkpath
  end

  # `brew services start agit` runs a local server on 127.0.0.1:7878 with data
  # in #{var}/agit. Set AGIT_SERVER_TOKEN in the plist/unit for authentication.
  service do
    run [opt_bin/"agit-server"]
    keep_alive true
    working_dir var/"agit"
    environment_variables AGIT_DATA_DIR: var/"agit", AGIT_LISTEN: "127.0.0.1:7878", RUST_LOG: "info"
    log_path var/"log/agit-server.log"
    error_log_path var/"log/agit-server.log"
  end

  def caveats
    <<~EOS
      Start a local server with:
        brew services start agit
      Data: #{var}/agit. Deployment and security notes:
        https://github.com/agit-vcs/agit/blob/main/docs/DEPLOYMENT.md
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/agit --version")
    system bin/"agit", "init"
    assert_predicate testpath/".agit", :exist?
    assert_match "agit-server", shell_output("#{bin}/agit-server --help")
  end
end
