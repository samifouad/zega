"""Start the released CLI binary and exercise its authenticated HTTP API."""

import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import Request, urlopen

Path(".tmp").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(dir=".tmp") as directory:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    token = Path(directory).resolve() / "token"
    token.write_text("local-smoke-fixture\n")
    server = subprocess.Popen([str(Path(sys.argv[1]).resolve()), "start", "--data", str(Path(directory).resolve() / "data"),
                               "--port", str(port), "--token-file", str(token)])
    try:
        request = Request(f"http://127.0.0.1:{port}/health", headers={"Authorization": "Bearer local-smoke-fixture"})
        for attempt in range(100):
            if server.poll() is not None:
                raise RuntimeError("Built CLI exited before becoming healthy")
            try:
                with urlopen(request, timeout=1) as response:
                    assert response.status == 200
                    assert json.load(response) == {"ok": True}
                    print("Built CLI: authenticated /health HTTP 200")
                    break
            except URLError:
                time.sleep(0.1)
        else:
            raise RuntimeError("Built CLI did not become healthy")
        request = Request(f"http://127.0.0.1:{port}/zql", data=json.dumps({"schema": "type Release { answer: Int }", "query": "mutation { Release(answer: 42) { answer } }"}).encode(),
                          headers={"Authorization": "Bearer local-smoke-fixture", "Content-Type": "application/json"})
        with urlopen(request, timeout=5) as response:
            result = json.load(response)
            assert result == {"ok": True, "result": {"answer": 42}}, result
            print("Built CLI: /zql returned answer=42")
    finally:
        server.terminate()
        server.wait(timeout=10)
