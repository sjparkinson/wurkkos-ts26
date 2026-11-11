#!/usr/bin/env ruby
# PostToolUse hook on Edit and Write. Lints the file that changed and, on a
# problem, exits 2 so stderr goes back to Claude to fix.
#
#   .rs  rustfmt --check, then clippy: the host's, and the AVR target's too for
#        files with code the host never compiles (src/avr/, which gets only
#        the AVR's, and the firmware and link probe mains)
#   .rb  ruby -wc
#   .github/workflows/*.yml  gh actions-lock --verify-local (lockfile coverage)

require "json"
require "open3"

# Everything in here is AVR-only, so the host build never sees it.
AVR_ONLY = "src/avr/"
# AVR code beside a host stub, so these want both clippies.
AVR_TOO = %w[src/main.rs examples/link.rs].freeze

def run(*cmd, chdir:)
  out, status = Open3.capture2e(*cmd, chdir: chdir)
  status.success? ? nil : "$ #{cmd.join(' ')}\n#{out.lines.last(80).join}"
end

def rust(path, dir)
  relative = path.delete_prefix("#{dir}/")
  fmt = run("rustfmt", "--edition", "2024", "--check", "--color", "never", path, chdir: dir)
  return fmt if fmt

  avr_only = relative.start_with?(AVR_ONLY)
  host = avr_only ? nil : run("cargo", "clippy", "--workspace", "--all-targets", "--locked", "-q",
                              "--", "-D", "warnings", chdir: dir)
  return host if host || !(avr_only || AVR_TOO.include?(relative))

  run("cargo", "clippy", "-q", "--package", "ts26", "--lib", "--bins", "--examples",
      "--release", "--target", "avr-none", "-Zbuild-std=core", "--", "-D", "warnings", chdir: dir)
end

input = JSON.parse($stdin.read)
path = input.dig("tool_input", "file_path").to_s
exit 0 if path.empty? || !File.file?(path)

# The checkout the file lives in, so edits in a worktree lint that worktree.
top, status = Open3.capture2("git", "-C", File.dirname(path), "rev-parse", "--show-toplevel")
dir = status.success? ? top.chomp : Dir.pwd
problem =
  if path.start_with?("#{dir}/.github/workflows/") && path.end_with?(".yml", ".yaml")
    run("gh", "actions-lock", "--verify-local", chdir: dir)
  else
    case File.extname(path)
    when ".rs" then rust(path, dir)
    when ".rb" then run("ruby", "-wc", path, chdir: dir)
    end
  end

exit 0 unless problem

warn("Lint problems after editing #{path}:\n#{problem}")
exit 2
