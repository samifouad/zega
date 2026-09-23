"""Exercise the production entry points against isolated Git and R2 fixtures.

No GitHub refs, credentials, registry or real buckets are used by these proofs.
"""

import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

from scripts import channels

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(os.environ.get("CHANNELS_SCRIPT", ROOT / "scripts/channels.py")).resolve()


class Fixture(unittest.TestCase):
    def setUp(self):
        (ROOT / ".tmp").mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / ".tmp")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                    "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                    "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"}
        self.git("init", "-q")
        (self.repo / "Cargo.toml").write_text('[workspace.package]\nversion = "1.2.3"\n')
        self.git("add", "Cargo.toml")
        self.git("commit", "-qm", "fixture")
        self.commit = self.git("rev-parse", "HEAD")
        self.tag = f"v1.2.3-canary-{self.commit[:7]}"
        self.remote = self.root / "remote.git"
        self.git("init", "--bare", "-q", str(self.remote))
        self.git("remote", "add", "origin", str(self.remote))
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.store = self.root / "r2"
        self.store.mkdir()
        self.log = self.root / "operations.jsonl"
        self.env.update(PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}",
                        FIXTURE_STORE=str(self.store), FIXTURE_LOG=str(self.log),
                        R2_ENDPOINT="https://fixture.invalid", GITHUB_REPOSITORY="fixture/repo")
        self.executable("aws", '''import json, os, pathlib, shutil, sys
args = sys.argv[1:]
with open(os.environ['FIXTURE_LOG'], 'a') as f: f.write(json.dumps(args) + '\\n')
assert args[:2] == ['s3', 'cp'], args
recursive = '--recursive' in args
paths = [a for a in args[2:args.index('--endpoint-url')] if a != '--recursive']
def local(value):
    return pathlib.Path(os.environ['FIXTURE_STORE']) / value[5:] if value.startswith('s3://') else pathlib.Path(value)
source, dest = map(local, paths)
if recursive:
    shutil.copytree(source, dest, dirs_exist_ok=True)
else:
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, dest)
''')
        self.executable("gh", '''import json, os, sys
with open(os.environ['FIXTURE_LOG'], 'a') as f: f.write(json.dumps(sys.argv[1:]) + '\\n')
''')

    def executable(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n{body}")
        path.chmod(0o755)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.repo, env=self.env, text=True, stderr=subprocess.PIPE).strip()

    def command(self, *args):
        return subprocess.run([sys.executable, str(SCRIPT), *args], cwd=self.repo, env=self.env, text=True, capture_output=True)

    def remote_tags(self):
        return self.git("ls-remote", "--tags", "origin")

    def fixture_canary(self):
        self.git("tag", self.tag)
        for bucket in channels.BUCKETS:
            dest = self.store / bucket / self.tag
            dest.mkdir(parents=True)
            (dest / "zega.wasm").write_bytes(b"\0asm-fixture-payload")
            with tarfile.open(dest / "package.tgz", "w:gz") as archive:
                for name, payload in {
                    "package/package.json": json.dumps({"name": "zegadb", "version": f"1.2.3-canary.{self.commit[:7]}", "exports": "./index.js"}).encode(),
                    "package/index.js": b"export const answer = 42;\n",
                    "package/engine.wasm": b"\0asm-fixture-payload",
                }.items():
                    member = tarfile.TarInfo(name)
                    member.size = len(payload)
                    archive.addfile(member, io.BytesIO(payload))
            data = dict(version=self.tag[1:], tag=self.tag, channel="canary", promoted_from=None,
                        commit=self.commit, base_version="1.2.3", artifacts={p.name: channels.digest(p) for p in dest.iterdir()})
            for name in channels.METADATA:
                channels.write_json(dest / name, data)
            latest = self.store / bucket / "latest"
            latest.mkdir()
            (latest / "sentinel").write_text("previous stable")

    def snapshot(self):
        return {str(p.relative_to(self.store)): p.read_bytes() for p in self.store.rglob("*") if p.is_file()}


class TagGuards(Fixture):
    def test_refuses_promoted_version(self):
        self.git("tag", "v1.2.3")
        result = self.command("tag-canary")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already promoted", result.stdout)
        self.assertEqual(self.git("tag"), "v1.2.3")
        self.assertEqual(self.remote_tags(), "")
        self.assertFalse(self.log.exists(), "Refusal must not dispatch a release")
        print(result.stdout.strip())

    def test_refuses_duplicate_tag(self):
        self.git("tag", self.tag)
        result = self.command("tag-canary")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already exists", result.stdout)
        self.assertEqual(self.remote_tags(), "")
        self.assertFalse(self.log.exists(), "Duplicate must not dispatch a rebuild")
        print(result.stdout.strip())

    def test_fresh_canary_pushes_exact_tag_and_dispatches(self):
        result = self.command("tag-canary")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"refs/tags/{self.tag}", self.remote_tags())
        self.assertEqual(self.git("rev-parse", f"{self.tag}^{{commit}}"), self.commit)
        self.assertEqual(json.loads(self.log.read_text()), ["workflow", "run", "release.yml", "--repo", "fixture/repo", "--ref", self.tag])


class Promotion(Fixture):
    def test_rejects_release_commit_mismatch_before_any_write(self):
        self.fixture_canary()
        path = self.store / "zega-releases" / self.tag / "release.json"
        data = channels.read_json(path)
        data["commit"] = "f" * 40
        channels.write_json(path, data)
        before = self.snapshot()
        result = self.command("promote", self.tag, "promotion")
        self.assertNotEqual(result.returncode, 0, "Mismatched canary was promoted")
        self.assertIn("release.json commit mismatch", result.stderr)
        self.assertEqual(self.snapshot(), before, "Integrity rejection must not mutate R2")
        self.assertEqual(self.remote_tags(), "")
        self.assertNotIn("v1.2.3", self.git("tag").splitlines())
        operations = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(len(operations), 1, "Mismatch must stop after the release.json read")
        print(result.stderr.strip())

    def test_promotes_same_bytes_and_repacks_only_npm_version(self):
        self.fixture_canary()
        # Promotion must tag the selected commit, even when main has advanced.
        (self.repo / "newer").write_text("new main")
        self.git("add", "newer")
        self.git("commit", "-qm", "new main")
        result = self.command("promote", self.tag, "promotion")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "v1.2.3^{commit}"), self.commit)
        self.assertIn("refs/tags/v1.2.3", self.remote_tags())
        for bucket in channels.BUCKETS:
            canary = self.store / bucket / self.tag
            stable = self.store / bucket / "v1.2.3"
            for source in canary.iterdir():
                target = stable / source.name
                if source.name in channels.METADATA:
                    expected = channels.read_json(source)
                    expected.update(version="1.2.3", tag="v1.2.3", channel="stable", promoted_from=self.tag)
                    self.assertEqual(channels.read_json(target), expected)
                else:
                    self.assertEqual(target.read_bytes(), source.read_bytes())
                self.assertEqual((self.store / bucket / "latest" / source.name).read_bytes(), target.read_bytes())
        def contents(path):
            with tarfile.open(path) as archive:
                return {m.name: archive.extractfile(m).read() for m in archive.getmembers()}
        canary = contents(self.store / "zega-releases" / self.tag / "package.tgz")
        stable = contents(self.repo / "promotion/package.tgz")
        before = json.loads(canary.pop("package/package.json"))
        after = json.loads(stable.pop("package/package.json"))
        before["version"] = "1.2.3"
        self.assertEqual(before, after)
        self.assertEqual(canary, stable)
        print("PROMOTED: R2 payloads byte-identical; npm contents differ only by package.json version")

    def test_corrupt_wasm_rejected_without_writes(self):
        self.fixture_canary()
        (self.store / "zega-wasm" / self.tag / "zega.wasm").write_bytes(b"corrupted")
        before = self.snapshot()
        result = self.command("promote", self.tag, "promotion")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.remote_tags(), "")

    def test_already_promoted_refused(self):
        self.fixture_canary()
        self.git("tag", "v1.2.3")
        result = self.command("promote", self.tag, "promotion")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already promoted", result.stderr)
        self.assertFalse(self.log.exists())


class Release(Fixture):
    def test_resolve_derives_npm_channel_and_rejects_stable_or_mismatched_tag(self):
        self.git("tag", self.tag)
        result = self.command("resolve", self.tag)
        self.assertEqual(result.returncode, 0, result.stderr)
        info = json.loads(result.stdout)
        self.assertEqual(info["npm_version"], f"1.2.3-canary.{self.commit[:7]}")
        self.assertEqual(info["commit"], self.commit)
        self.assertNotEqual(self.command("resolve", "v1.2.3").returncode, 0)
        bad_tag = "v1.2.3-canary-" + ("0000000" if self.commit[:7] != "0000000" else "1111111")
        self.git("tag", bad_tag)
        self.assertNotEqual(self.command("resolve", bad_tag).returncode, 0)
        self.git("tag", "v1.2.3")
        self.assertNotEqual(self.command("resolve", self.tag).returncode, 0)

    def test_prepare_binds_native_wasm_and_tested_npm_to_commit(self):
        self.git("tag", self.tag)
        (self.repo / "Cargo.lock").write_text("fixture lockfile")
        artifacts = self.repo / "artifacts"
        wasm = artifacts / "npm-package/wasm"
        wasm.mkdir(parents=True)
        (wasm / "engine.wasm").write_bytes(b"\0asm-fixture")
        for platform in ("linux-x64", "darwin-arm64", "darwin-x64", "windows-x64"):
            native = artifacts / f"native-{platform}"
            native.mkdir()
            name = f"zega-{platform}" + (".exe" if platform == "windows-x64" else "")
            (native / name).write_bytes(platform.encode())
        with tarfile.open(artifacts / "npm-package/package.tgz", "w:gz") as archive:
            for name, payload in {
                "package/package.json": json.dumps({"name": "zegadb", "version": f"1.2.3-canary.{self.commit[:7]}"}).encode(),
                "package/wasm/engine.wasm": b"\0asm-fixture",
            }.items():
                member = tarfile.TarInfo(name)
                member.size = len(payload)
                archive.addfile(member, io.BytesIO(payload))
        result = self.command("prepare", self.tag, "artifacts", "release")
        self.assertEqual(result.returncode, 0, result.stderr)
        channels.verify(self.repo / "release", self.tag, self.commit)
        self.assertEqual(len(channels.read_json(self.repo / "release/release.json")["artifacts"]), 6)
        inventory = channels.read_json(self.repo / "release/manifest.json")["artifacts"]
        for name in ("zega-linux-x64", "zega-darwin-arm64", "zega-darwin-x64", "zega-windows-x64.exe"):
            self.assertEqual(inventory[name], channels.digest(self.repo / "release" / name))
        self.assertFalse(any(name.startswith("zega-server-") for name in inventory))
        (wasm / "engine.wasm").write_bytes(b"different-from-tested-package")
        result = self.command("prepare", self.tag, "artifacts", "bad-release")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("R2 WASM must match", result.stderr)


if __name__ == "__main__":
    unittest.main()
