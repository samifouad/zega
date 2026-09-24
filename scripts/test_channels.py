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
from unittest.mock import patch

from scripts import channels

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(os.environ.get("CHANNELS_SCRIPT", ROOT / "scripts/channels.py")).resolve()


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        (self.repo / ".tmp").mkdir()
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
if args[0] == 's3api':
    bucket = args[args.index('--bucket') + 1]
    prefix = args[args.index('--prefix') + 1]
    target = pathlib.Path(os.environ['FIXTURE_STORE']) / bucket / prefix
    if args[1] == 'list-objects-v2' and 'Contents[].Key' in args:
        keys = [p.relative_to(pathlib.Path(os.environ['FIXTURE_STORE']) / bucket).as_posix()
                for p in sorted(target.rglob('*')) if p.is_file()] if target.exists() else []
        print(json.dumps(keys))
    else:
        print('1' if target.exists() and any(target.rglob('*')) else '0')
    raise SystemExit(0)
assert args[:1] == ['s3'] and args[1] in ('cp', 'sync', 'rm'), args
operation = args[1]
paths = [a for a in args[2:args.index('--endpoint-url')] if a not in ('--recursive', '--delete')]
def local(value):
    return pathlib.Path(os.environ['FIXTURE_STORE']) / value[5:] if value.startswith('s3://') else pathlib.Path(value)
if operation == 'rm':
    local(paths[0]).unlink()
    raise SystemExit(0)
source, dest = map(local, paths)
if operation == 'sync':
    if '--delete' in args and dest.exists(): shutil.rmtree(dest)
    shutil.copytree(source, dest, dirs_exist_ok=True)
elif '--recursive' in args:
    shutil.copytree(source, dest, dirs_exist_ok=True)
    if source.name == 'canary' and source.is_relative_to(pathlib.Path(os.environ['FIXTURE_STORE'])) and os.environ.get('CORRUPT_READBACK'):
        next(dest.rglob('zega.wasm')).write_bytes(b'corrupted-readback')
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
            self.create_canary(dest)
            latest = self.store / bucket / "latest"
            latest.mkdir()
            (latest / "sentinel").write_text("previous stable")

    def create_canary(self, dest):
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

    def assert_no_bucket_copies(self):
        operations = [json.loads(line) for line in self.log.read_text().splitlines()]
        for operation in operations:
            s3_paths = [arg for arg in operation if arg.startswith("s3://")]
            self.assertLessEqual(len(s3_paths), 1, f"Bucket-to-bucket S3 operation: {operation}")
        return operations

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
        operations = self.assert_no_bucket_copies()
        for bucket in channels.BUCKETS:
            local_source = (self.repo / "promotion" / bucket).resolve()
            sync = next(operation for operation in operations
                        if operation[:2] == ["s3", "sync"] and f"s3://{bucket}/v1.2.3/" in operation)
            self.assertIn("--delete", sync)
            self.assertEqual((self.repo / sync[2].rstrip("/")).resolve(), local_source)
            self.assertFalse(any(operation[:2] == ["s3", "sync"] and f"s3://{bucket}/latest/" in operation
                                 for operation in operations))
            latest_upload = next(i for i, operation in enumerate(operations)
                                 if operation[:2] == ["s3", "cp"] and "--recursive" in operation
                                 and f"s3://{bucket}/latest/" in operation)
            latest_readback = next(i for i, operation in enumerate(operations)
                                   if operation[:2] == ["s3", "cp"] and "--recursive" in operation
                                   and f"s3://{bucket}/latest/" in operation and i > latest_upload)
            latest_json = next(i for i, operation in enumerate(operations)
                               if operation[:2] == ["s3", "cp"] and f"s3://{bucket}/latest.json" in operation)
            self.assertLess(latest_readback, latest_json)
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


class Publication(Fixture):
    def test_r2_errors_name_operation_and_prefix_without_diagnostics(self):
        result = subprocess.CompletedProcess([], 1, stdout=b"", stderr=b"private endpoint and credentials")
        with patch.dict(os.environ, {"R2_ENDPOINT": "https://fixture.invalid"}):
            with patch("scripts.channels.subprocess.run", return_value=result):
                with self.assertRaisesRegex(RuntimeError, r"R2 sync failed for zega-releases/canary/ \(diagnostics withheld\)") as error:
                    channels.aws("sync", "release/", "s3://zega-releases/canary/", "--delete")
        self.assertNotIn("private endpoint", str(error.exception))
        self.assertNotIn("credentials", str(error.exception))

    def test_canary_pointer_uploads_all_bytes_deletes_only_stale_and_reads_back_before_json(self):
        self.git("tag", self.tag)
        release = self.repo / "release"
        self.create_canary(release)
        for bucket in channels.BUCKETS:
            stale = self.store / bucket / "canary"
            stale.mkdir(parents=True)
            (stale / "zega-server-old").write_bytes(b"stale binary")
            # Same-size stale content with a newer mtime is the case `s3 sync` can skip.
            (stale / "zega.wasm").write_bytes(b"X" * (release / "zega.wasm").stat().st_size)
            os.utime(stale / "zega.wasm", (2_000_000_000, 2_000_000_000))
        result = self.command("publish-r2", self.tag, "release")
        self.assertEqual(result.returncode, 0, result.stderr)
        operations = self.assert_no_bucket_copies()
        for bucket in channels.BUCKETS:
            pointer = self.store / bucket / "canary"
            self.assertEqual({p.name for p in pointer.iterdir()}, {p.name for p in release.iterdir()})
            self.assertEqual((pointer / "zega.wasm").read_bytes(), (release / "zega.wasm").read_bytes())
            self.assertEqual(channels.read_json(self.store / bucket / "canary.json"), channels.read_json(release / "release.json"))
            self.assertFalse(any(operation[:2] == ["s3", "sync"] for operation in operations))
            upload = next(operation for operation in operations
                          if operation[:2] == ["s3", "cp"] and "--recursive" in operation
                          and f"s3://{bucket}/canary/" in operation)
            local_source = next(path for path in upload[2:] if not path.startswith("s3://") and path != "--recursive")
            self.assertEqual((self.repo / local_source).resolve(), release.resolve())
            deletes = [operation for operation in operations if operation[:2] == ["s3", "rm"]
                       and f"s3://{bucket}/canary/" in operation[2]]
            self.assertEqual([operation[2] for operation in deletes], [f"s3://{bucket}/canary/zega-server-old"])
            readback = next(i for i, operation in enumerate(operations)
                            if operation[:2] == ["s3", "cp"] and "--recursive" in operation
                            and f"s3://{bucket}/canary/" in operation)
            pointer_json = next(i for i, operation in enumerate(operations)
                                if operation[:2] == ["s3", "cp"] and f"s3://{bucket}/canary.json" in operation)
            readback_copy = next(i for i, operation in enumerate(operations)
                                 if operation[:2] == ["s3", "cp"] and "--recursive" in operation
                                 and f"s3://{bucket}/canary/" in operation and i > readback)
            self.assertLess(readback_copy, pointer_json)

    def test_canary_readback_mismatch_aborts_before_json_pointer_write(self):
        self.git("tag", self.tag)
        release = self.repo / "release"
        self.create_canary(release)
        env = {**self.env, "CORRUPT_READBACK": "1"}
        result = subprocess.run([sys.executable, str(SCRIPT), "publish-r2", self.tag, "release"],
                                cwd=self.repo, env=env, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0, "Corrupt pointer readback was accepted")
        self.assertIn("checksum mismatch", result.stderr)
        operations = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(operation[:2] == ["s3", "cp"] and
                             any(path in operation for path in (f"s3://{bucket}/canary.json" for bucket in channels.BUCKETS))
                             for operation in operations), "canary.json was written after a failed readback")

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
