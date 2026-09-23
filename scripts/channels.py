#!/usr/bin/env python3
"""Release-channel operations shared by the workflows and their executable proofs."""

import argparse
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib


CANARY = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-canary-([0-9a-f]{7})")
BUCKETS = ("zega-releases", "zega-wasm")
METADATA = ("manifest.json", "release.json")


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def git(*args):
    return run("git", *args)


def version():
    with open("Cargo.toml", "rb") as source:
        value = tomllib.load(source)["workspace"]["package"]["version"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("workspace.package.version must be a stable X.Y.Z version")
    return value


def has_tag(tag):
    return subprocess.run(["git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}"]).returncode == 0


def identity(tag):
    match = CANARY.fullmatch(tag)
    if not match:
        raise ValueError("Expected vX.Y.Z-canary-<7-char-sha>")
    base = ".".join(match.group(1, 2, 3))
    commit = git("rev-parse", f"refs/tags/{tag}^{{commit}}")
    if commit[:7] != match[4]:
        raise ValueError("Canary tag suffix does not match the tagged commit")
    tree = tomllib.loads(git("show", f"{commit}:Cargo.toml"))
    if tree["workspace"]["package"]["version"] != base:
        raise ValueError("Canary tag does not match the workspace version at its commit")
    return base, commit


def output(**values):
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a") as target:
            for key, value in values.items():
                target.write(f"{key}={value}\n")
    print(json.dumps(values, sort_keys=True))


def tag_canary():
    base = version()
    tag = f"v{base}-canary-{git('rev-parse', 'HEAD')[:7]}"
    if has_tag(f"v{base}"):
        print(f"REFUSED: v{base} is already promoted; bump the workspace version")
        return
    if has_tag(tag):
        print(f"REFUSED: canary tag {tag} already exists; nothing to do")
        return
    git("tag", "-a", tag, "-m", f"zega canary {tag}")
    git("push", "origin", f"refs/tags/{tag}")
    # GITHUB_TOKEN tag pushes do not start workflows. Explicit dispatch does.
    run("gh", "workflow", "run", "release.yml", "--repo", os.environ["GITHUB_REPOSITORY"], "--ref", tag)
    output(tag=tag)


def resolve(tag):
    base, commit = identity(tag)
    if git("rev-parse", "HEAD") != commit:
        raise ValueError("Release checkout must be the canary commit")
    if has_tag(f"v{base}"):
        raise ValueError(f"v{base} is already promoted")
    output(tag=tag, version=tag[1:], base_version=base, commit=commit,
           npm_version=f"{base}-canary.{commit[:7]}")


def read_json(path):
    return json.loads(Path(path).read_text())


def write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=2) + "\n")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def repack(source, destination, expected, target):
    """Change only package/package.json's version; preserve every other file byte."""
    with tarfile.open(source, "r:gz") as before, tarfile.open(destination, "w:gz") as after:
        seen = set()
        for member in before.getmembers():
            if not member.isfile() or not member.name.startswith("package/") or ".." in Path(member.name).parts:
                raise ValueError("npm archive contains an unexpected path or non-file entry")
            if member.name in seen:
                raise ValueError("npm archive contains duplicate entries")
            seen.add(member.name)
            payload = before.extractfile(member).read()
            if member.name == "package/package.json":
                data = json.loads(payload)
                if data["name"] != "zegadb" or data["version"] != expected:
                    raise ValueError("npm archive identity does not match this channel")
                data["version"] = target
                payload = (json.dumps(data, indent=2) + "\n").encode()
            member = copy.copy(member)
            member.size = len(payload)
            after.addfile(member, io.BytesIO(payload))
        if "package/package.json" not in seen:
            raise ValueError("npm archive has no package.json")


def prepare(tag, artifacts, destination):
    base, commit = identity(tag)
    source, dest = Path(artifacts), Path(destination)
    dest.mkdir(parents=True, exist_ok=False)
    for platform in ("linux-x64", "darwin-arm64", "darwin-x64", "windows-x64"):
        name = f"zega-server-{platform}" + (".exe" if platform == "windows-x64" else "")
        shutil.copyfile(source / f"native-{platform}" / name, dest / name)
    shutil.copytree(source / "npm-package" / "wasm", dest / "wasm")
    shutil.copyfile(source / "npm-package" / "package.tgz", dest / "package.tgz")
    # Check the package identity before making this release promotable.
    with tarfile.open(dest / "package.tgz") as archive:
        package = json.load(archive.extractfile("package/package.json"))
        for path in (dest / "wasm").iterdir():
            if archive.extractfile(f"package/wasm/{path.name}").read() != path.read_bytes():
                raise ValueError("R2 WASM must match the tested npm package bytes")
    if package["name"] != "zegadb" or package["version"] != f"{base}-canary.{commit[:7]}":
        raise ValueError("Release package must be the tested zegadb canary")
    data = dict(version=tag[1:], tag=tag, base_version=base, commit=commit,
                channel="canary", promoted_from=None,
                cargo_lock_sha256=digest("Cargo.lock"),
                artifacts={p.relative_to(dest).as_posix(): digest(p) for p in sorted(dest.rglob("*")) if p.is_file()})
    for name in METADATA:
        write_json(dest / name, data)


def aws(*args):
    # Avoid ever displaying credential-bearing endpoint/CLI diagnostics.
    result = subprocess.run(["aws", "s3", *args, "--endpoint-url", os.environ["R2_ENDPOINT"], "--only-show-errors"],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError("R2 operation failed; check bucket access and object existence (diagnostics withheld)")


def check_commit(directory, tag, commit):
    release = read_json(Path(directory) / "release.json")
    if release["commit"] != commit:
        raise ValueError(f"release.json commit mismatch: tag {tag} points at {commit}, artifacts record {release['commit']}")
    return release


def verify(directory, tag, commit):
    directory = Path(directory)
    # This check MUST precede any object copy, tag creation or pointer mutation.
    release = check_commit(directory, tag, commit)
    for name in METADATA:
        data = read_json(directory / name)
        if data["tag"] != tag or data["commit"] != commit or data["channel"] != "canary":
            raise ValueError(f"{name} is not the selected canary")
        if data["artifacts"] != release["artifacts"]:
            raise ValueError("Metadata checksum inventories disagree")
    files = {p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file()} - set(METADATA)
    if not files or files != set(release["artifacts"]):
        raise ValueError("Canary payload inventory is incomplete or has unexpected objects")
    for name, expected in release["artifacts"].items():
        if digest(directory / name) != expected:
            raise ValueError(f"Canary checksum mismatch: {name}")


def publish_r2(tag, directory):
    base, commit = identity(tag)
    if has_tag(f"v{base}"):
        raise ValueError(f"v{base} is already promoted")
    verify(directory, tag, commit)
    # Serialized with promotion. Never overwrite an existing immutable prefix.
    for bucket in BUCKETS:
        existing = subprocess.run(["aws", "s3api", "list-objects-v2", "--bucket", bucket, "--prefix", f"{tag}/",
                                   "--max-keys", "1", "--query", "KeyCount", "--output", "text", "--endpoint-url", os.environ["R2_ENDPOINT"]],
                                  capture_output=True, text=True)
        if existing.returncode:
            raise RuntimeError("Cannot check R2 prefix; check release credentials (diagnostics withheld)")
        if existing.stdout.strip() != "0":
            raise ValueError(f"Canary prefix already exists in {bucket}; refusing overwrite")
    for bucket in BUCKETS:
        aws("cp", "--recursive", f"{directory}/", f"s3://{bucket}/{tag}/")
    # Read back before advancing canary. latest is only touched by promote.
    for bucket in BUCKETS:
        check = Path(".tmp/readback") / bucket
        check.mkdir(parents=True, exist_ok=False)
        aws("cp", "--recursive", f"s3://{bucket}/{tag}/", f"{check}/")
        verify(check, tag, commit)
    for bucket in BUCKETS:
        aws("cp", "--recursive", f"s3://{bucket}/{tag}/", f"s3://{bucket}/canary/")
        aws("cp", f"{directory}/release.json", f"s3://{bucket}/canary.json")


def promote(tag, directory):
    base, commit = identity(tag)
    if has_tag(f"v{base}"):
        raise ValueError(f"v{base} is already promoted")
    work = Path(directory)
    work.mkdir(parents=True, exist_ok=False)
    for bucket in BUCKETS:
        source = work / bucket
        source.mkdir()
        # Fetch and compare release.json FIRST, before downloading any payload.
        aws("cp", f"s3://{bucket}/{tag}/release.json", str(source / "release.json"))
        check_commit(source, tag, commit)
        aws("cp", "--recursive", f"s3://{bucket}/{tag}/", f"{source}/")
        verify(source, tag, commit)
    repack(work / BUCKETS[0] / "package.tgz", work / "package.tgz", f"{base}-canary.{commit[:7]}", base)
    for bucket in BUCKETS:
        source = work / bucket
        aws("cp", "--recursive", f"s3://{bucket}/{tag}/", f"s3://{bucket}/v{base}/")
        for name in METADATA:
            data = read_json(source / name)
            data.update(version=base, tag=f"v{base}", channel="stable", promoted_from=tag)
            write_json(source / name, data)
            aws("cp", str(source / name), f"s3://{bucket}/v{base}/{name}")
        readback = work / f"{bucket}-readback"
        readback.mkdir()
        aws("cp", "--recursive", f"s3://{bucket}/v{base}/", f"{readback}/")
        expected = {p.relative_to(source): p.read_bytes() for p in source.rglob("*") if p.is_file()}
        actual = {p.relative_to(readback): p.read_bytes() for p in readback.rglob("*") if p.is_file()}
        if actual != expected:
            raise ValueError("Stable readback differs from the canary payload and rewritten metadata")
    git("tag", "-a", f"v{base}", commit, "-m", f"zega v{base} (promoted from {tag})")
    git("push", "origin", f"refs/tags/v{base}")
    for bucket in BUCKETS:
        aws("cp", "--recursive", f"s3://{bucket}/v{base}/", f"s3://{bucket}/latest/")
        aws("cp", f"s3://{bucket}/v{base}/release.json", f"s3://{bucket}/latest.json")
    output(tag=f"v{base}", version=base, commit=commit)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["tag-canary", "resolve", "prepare", "publish-r2", "promote", "repack"])
    parser.add_argument("args", nargs="*")
    args = parser.parse_args()
    commands = {"tag-canary": tag_canary, "resolve": resolve, "prepare": prepare,
                "publish-r2": publish_r2, "promote": promote, "repack": repack}
    try:
        commands[args.command](*args.args)
    except (ValueError, RuntimeError, KeyError, OSError, subprocess.CalledProcessError) as error:
        # CalledProcessError may include commands with endpoint values. Hide it.
        message = "External command failed" if isinstance(error, subprocess.CalledProcessError) else str(error)
        parser.exit(1, f"ERROR: {message}\n")


if __name__ == "__main__":
    main()
