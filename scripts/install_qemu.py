#!/usr/bin/env python3
"""Install QEMU on Ubuntu x86 CI hosts without an unbounded mirror download.

Bound only metadata/download phases, never interrupt dpkg to switch mirrors.
Fallback uses isolated, signed Ubuntu sources/lists; runner sources stay intact.
"""
import argparse
import pathlib
import platform
import re
import shlex
import subprocess
import tempfile

PACKAGES = {
    "x86_64": ("qemu-system-x86", "binutils", "gcc"),
    "riscv64": ("qemu-system-misc",),
    "aarch64": ("qemu-system-arm",),
}
APT_OPTIONS = [
    "-o", "Acquire::Retries=2",
    "-o", "Acquire::http::Timeout=30",
    "-o", "Acquire::https::Timeout=30",
    "-o", "APT::Update::Error-Mode=any",
]


def run_command(command):
    print(shlex.join(command), flush=True)
    return subprocess.run(command, check=False).returncode


def install(guest, release, host, run=run_command):
    codename = release.get("VERSION_CODENAME", "")
    if (release.get("ID") != "ubuntu" or host != "x86_64"
            or not re.fullmatch(r"[a-z][a-z0-9]*", codename)):
        raise ValueError("QEMU CI installer requires an Ubuntu x86_64 host and valid codename")
    packages = PACKAGES[guest]

    def apt(options, args, deadline=None):
        command = ["sudo"]
        if deadline:
            command += ["timeout", "--kill-after=5s", deadline]
        command += ["env", "DEBIAN_FRONTEND=noninteractive", "apt-get",
                    *APT_OPTIONS, *options, *args]
        return run(command)

    def download(options):
        if apt(options, ["update"], "60s") != 0:
            return False
        return apt(options, ["--download-only", "install", "-y",
                             "--no-install-recommends", *packages], "120s") == 0

    def finish(options):
        # All archives must already be present. No network retry or timeout may
        # interrupt dpkg here; a missing archive is a failure, not a new fetch.
        code = apt(options, ["--no-download", "install", "-y",
                             "--no-install-recommends", *packages])
        if code != 0:
            raise RuntimeError(f"offline QEMU package installation failed ({code})")
        code = run([f"qemu-system-{guest}", "--version"])
        if code != 0:
            raise RuntimeError(f"installed QEMU executable check failed ({code})")

    if download([]):
        finish([])
        return
    print("Runner mirror acquisition failed or timed out; trying signed Ubuntu archives", flush=True)
    # APT can leave root/_apt-owned files here. Keep this ephemeral CI state for
    # diagnostics; runner teardown removes it without a privileged cleanup step.
    root = pathlib.Path(tempfile.mkdtemp(prefix="aether-apt-"))
    print(f"Archive source state: {root}", flush=True)
    root.chmod(0o755)  # APT's _apt downloader must traverse the directory.
    lists = root / "lists"
    (lists / "partial").mkdir(parents=True)
    source = root / "sources.list"
    signed = "[signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg]"
    source.write_text(
        f"deb {signed} https://archive.ubuntu.com/ubuntu {codename} main universe\n"
        f"deb {signed} https://archive.ubuntu.com/ubuntu {codename}-updates main universe\n"
        f"deb {signed} https://security.ubuntu.com/ubuntu {codename}-security main universe\n",
        encoding="utf-8",
    )
    options = ["-o", f"Dir::Etc::sourcelist={source}",
               "-o", "Dir::Etc::sourceparts=-", "-o", f"Dir::State::lists={lists}"]
    if not download(options):
        raise RuntimeError("QEMU acquisition failed on runner and Ubuntu archive mirrors")
    finish(options)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("guest", choices=PACKAGES)
    args = parser.parse_args(argv)
    try:
        install(args.guest, platform.freedesktop_os_release(), platform.machine())
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"QEMU setup failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
