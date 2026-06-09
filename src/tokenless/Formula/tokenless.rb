# typed: false
# frozen_string_literal: true

# Homebrew formula for tokenless — LLM Token Optimization Toolkit
#
# Install from local source tarball (created by `make dist-brew`):
#   brew install --formula Formula/tokenless.rb
class Tokenless < Formula
  desc "LLM token optimization via schema/response compression, TOON encoding, and command rewriting"
  homepage "https://github.com/alibaba/anolisa"
  url "https://github.com/alibaba/anolisa/releases/download/tokenless/v0.5.0/tokenless-0.5.0.tar.gz"
  sha256 "273c7569c5cabd4eff552a3caad467460056840071447dfd0b46722a7cf5ed15"
  license "MIT"
  version "0.5.0"

  depends_on "rust" => :build
  depends_on "node" => :build
  depends_on "python@3.13"
  depends_on "jq"

  def install
    # Prevent Python bytecode (__pycache__/*.pyc) from being generated during
    # build or post-install processing inside the Homebrew sandbox.
    ENV["PYTHONDONTWRITEBYTECODE"] = "1"

    # Extract adapter resources from source tarball to a staging dir within
    # buildpath. Using a dedicated subdirectory avoids interference with the
    # main cargo build while staying inside the Homebrew sandbox.
    adapter_staging = buildpath/"adapter-staging"
    rm_rf adapter_staging
    mkdir_p adapter_staging
    system "tar", "xf", cached_download, "-C", adapter_staging.to_s,
           "tokenless/adapters"
    adapter_src = adapter_staging/"tokenless/adapters/tokenless"

    # Build tokenless workspace (tokenless-cli, tokenless-schema, tokenless-stats)
    system "cargo", "build", "--release"

    # Build rtk (vendored third-party, pre-patched in tarball)
    system "cargo", "build", "--release",
           "--manifest-path", "third_party/rtk/Cargo.toml"

    # Build toon (JSON to TOON encoder/decoder)
    system "cargo", "install", "toon-format",
           "--version", "0.4.6",
           "--root", buildpath/"toon-root",
           "--locked"

    # Stamp adapter template versions from Cargo.toml
    system "make", "stamp-adapter-templates"

    # Copy stamped config files (generated from .in templates) into staging area
    %w[manifest.json
       openclaw/package.json
       openclaw/openclaw.plugin.json
       hermes/plugin.yaml
       qoder/.qoder-plugin/plugin.json
       claude-code/.claude-plugin/plugin.json
       codex/.codex-plugin/plugin.json].each do |f|
      cp "adapters/tokenless/#{f}", adapter_src/"#{f}"
    end

    # Build OpenClaw TypeScript plugin -> dist/index.js
    cd "adapters/tokenless/openclaw" do
      system "npm", "install", "--legacy-peer-deps",
             "--no-audit", "--no-fund", "--package-lock=false"
      system "npm", "run", "build"
    end
    (adapter_src/"openclaw/dist").mkpath
    cp "adapters/tokenless/openclaw/dist/index.js", adapter_src/"openclaw/dist/index.js"

    # -- Install binaries --
    bin.install "target/release/tokenless"
    libexec.install "third_party/rtk/target/release/rtk"
    libexec.install buildpath/"toon-root/bin/toon"
    bin.install_symlink libexec/"rtk"
    bin.install_symlink libexec/"toon"

    # -- Install adapter resources (using cp, not .install, to avoid chdir issues) --
    adapter_dst = share/"anolisa/adapters/tokenless"

    # common
    safe_cp_r adapter_src/"common/hooks", adapter_dst/"common/hooks"
    safe_cp_r adapter_src/"common/commands", adapter_dst/"common/commands"
    safe_cp adapter_src/"manifest.json", adapter_dst/"manifest.json"
    safe_cp adapter_src/"common/tool-ready-spec.json", adapter_dst/"common/tool-ready-spec.json"
    safe_cp adapter_src/"common/cosh-extension.json", adapter_dst/"common/cosh-extension.json"
    safe_cp adapter_src/"common/tokenless-env-fix.sh", adapter_dst/"common/tokenless-env-fix.sh", executable: true
    chmod_scripts adapter_dst/"common/hooks"

    # openclaw
    (adapter_dst/"openclaw/dist").mkpath
    (adapter_dst/"openclaw/scripts").mkpath
    safe_cp adapter_src/"openclaw/openclaw.plugin.json", adapter_dst/"openclaw/openclaw.plugin.json"
    safe_cp adapter_src/"openclaw/package.json", adapter_dst/"openclaw/package.json"
    safe_cp adapter_src/"openclaw/dist/index.js", adapter_dst/"openclaw/dist/index.js"
    safe_cp_scripts adapter_src/"openclaw/scripts", adapter_dst/"openclaw/scripts"

    # hermes
    (adapter_dst/"hermes/scripts").mkpath
    safe_cp adapter_src/"hermes/__init__.py", adapter_dst/"hermes/__init__.py", executable: true
    safe_cp adapter_src/"hermes/plugin.yaml", adapter_dst/"hermes/plugin.yaml"
    safe_cp_scripts adapter_src/"hermes/scripts", adapter_dst/"hermes/scripts"

    # qoder
    (adapter_dst/"qoder/.qoder-plugin").mkpath
    (adapter_dst/"qoder/scripts").mkpath
    (adapter_dst/"qoder/commands").mkpath
    safe_cp adapter_src/"qoder/.qoder-plugin/plugin.json", adapter_dst/"qoder/.qoder-plugin/plugin.json"
    safe_cp adapter_src/"qoder/hooks.json", adapter_dst/"qoder/hooks.json"
    safe_cp adapter_src/"qoder/commands/tokenless-stats.toml", adapter_dst/"qoder/commands/tokenless-stats.toml"
    safe_cp_scripts adapter_src/"qoder/scripts", adapter_dst/"qoder/scripts"

    # claude-code
    (adapter_dst/"claude-code/.claude-plugin").mkpath
    (adapter_dst/"claude-code/hooks").mkpath
    (adapter_dst/"claude-code/scripts").mkpath
    safe_cp adapter_src/"claude-code/.claude-plugin/marketplace.json", adapter_dst/"claude-code/.claude-plugin/marketplace.json"
    safe_cp adapter_src/"claude-code/.claude-plugin/plugin.json", adapter_dst/"claude-code/.claude-plugin/plugin.json"
    safe_cp adapter_src/"claude-code/hooks/hooks.json", adapter_dst/"claude-code/hooks/hooks.json"
    safe_cp adapter_src/"claude-code/hooks/run-hook.sh", adapter_dst/"claude-code/hooks/run-hook.sh", executable: true
    safe_cp_scripts adapter_src/"claude-code/scripts", adapter_dst/"claude-code/scripts"

    # codex
    (adapter_dst/"codex/hooks").mkpath
    (adapter_dst/"codex/scripts").mkpath
    safe_cp adapter_src/"codex/hooks/hooks.json", adapter_dst/"codex/hooks/hooks.json"
    safe_cp_scripts adapter_src/"codex/scripts", adapter_dst/"codex/scripts"

    # -- Install cosh extension (auto-discovered by copilot-shell) --
    ext_dst = share/"anolisa/extensions/tokenless"
    safe_cp adapter_src/"common/cosh-extension.json", ext_dst/"cosh-extension.json"
    safe_cp adapter_src/"common/tool-ready-spec.json", ext_dst/"tool-ready-spec.json"
    safe_cp adapter_src/"common/tokenless-env-fix.sh", ext_dst/"tokenless-env-fix.sh", executable: true
    safe_cp_r adapter_src/"common/hooks", ext_dst/"hooks"
    safe_cp_r adapter_src/"common/commands", ext_dst/"commands"
    chmod_scripts ext_dst
    chmod_scripts ext_dst/"hooks"

    # -- Install documentation --
    doc.install "docs/tokenless-user-manual-en.md"
    doc.install "docs/tokenless-user-manual-zh.md"
    doc.install "docs/response-compression.md"
    doc.install "LICENSE"

    # Cleanup staging area and Python bytecode.
    rm_rf adapter_staging
    # Homebrew's post-install processing may generate __pycache__ after the
    # Ruby install method completes.  The post_install hook cleans up afterwards.
  end

  def post_install
    # Final cleanup: remove any __pycache__ generated during install or test
    # phases (Homebrew sandbox may trigger Python bytecode compilation).
    # Use find to recursively catch all __pycache__ directories under share/.
    system "find", share.to_s, "-name", "__pycache__", "-type", "d", "-delete"
  end

  test do
    ENV["PYTHONDONTWRITEBYTECODE"] = "1"
    assert_match version.to_s, shell_output("#{bin}/tokenless --version")

    # Verify rtk and toon symlinks resolve and run
    assert_match "rtk", shell_output("#{bin}/rtk --version 2>&1")
    assert_match "toon", shell_output("#{bin}/toon --version 2>&1")

    # Verify compression works
    input = '{"name":"test","debug":null,"metadata":{"created":"2024-01-01","updated":null}}'
    output = pipe_output("#{bin}/tokenless compress-response", input)
    assert_match "test", output
    refute_match "null", output

    # Verify TOON encoding
    toon_output = pipe_output("#{bin}/tokenless compress-toon", '{"a":1,"b":[2,3]}')
    assert_match "a:", toon_output
  end

  def caveats
    <<~EOS
      Adapter resources installed to:
        #{share}/anolisa/adapters/tokenless/

      To register adapters with your AI coding tools, run:
        # Claude Code
        #{share}/anolisa/adapters/tokenless/claude-code/scripts/install.sh

        # Copilot Shell (cosh) — auto-discovered from:
        #{share}/anolisa/extensions/tokenless/

      Hook scripts require python3 in PATH. If using the Homebrew-provided
      Python, ensure it is linked or in PATH:
        brew link python@3.13
        # or: export PATH="#{Formula["python@3.13"].opt_bin}:$PATH"

      For the full user manual:
        #{doc}/tokenless-user-manual-en.md
    EOS
  end

  private

  # Copy a single file using system cp (avoids Homebrew Pathname#install chdir issues)
  def safe_cp(src, dst, executable: false)
    dst.dirname.mkpath unless dst.dirname.exist?
    cp src.to_s, dst.to_s
    dst.chmod(0755) if executable
  end

  # Copy a directory tree using system cp -r
  def safe_cp_r(src, dst)
    dst.dirname.mkpath unless dst.dirname.exist?
    rm_rf dst if dst.exist?
    cp_r src.to_s, dst.to_s
  end

  # Copy all .sh files from src_dir to dst_dir, making them executable
  def safe_cp_scripts(src_dir, dst_dir)
    dst_dir.mkpath unless dst_dir.exist?
    Dir[src_dir/"*.sh"].each do |f|
      cp f.to_s, (dst_dir/File.basename(f)).to_s
      (dst_dir/File.basename(f)).chmod(0755)
    end
  end

  # Make all .sh and .py files executable
  def chmod_scripts(dir)
    return unless dir.exist?
    Dir[dir/"*.{sh,py}"].each { |f| File.chmod(0755, f) }
  end
end
