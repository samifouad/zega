"""Start the released server binary and exercise its authenticated HTTP API."""

import json
import os
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
    env = {**os.environ, "ZEGA_DATA": str(Path(directory).resolve() / "data"),
           "ZEGA_SERVER_TOKEN": "local-smoke-fixture", "ZEGA_SERVER_WORKERS": "2",
           "ZEGA_SERVER_ADDR": f"127.0.0.1:{port}"}
    server = subprocess.Popen([str(Path(sys.argv[1]).resolve())], env=env)
    try:
        request = Request(f"http://127.0.0.1:{port}/health", headers={"Authorization": "Bearer local-smoke-fixture"})
        for attempt in range(100):
            if server.poll() is not None:
                raise RuntimeError("Built server exited before becoming healthy")
            try:
                with urlopen(request, timeout=1) as response:
                    assert response.status == 200
                    assert json.load(response) == {"ok": True}
                    print("Built server: authenticated /health HTTP 200")
                    break
            except URLError:
                time.sleep(0.1)
        else:
            raise RuntimeError("Built server did not become healthy")
        request = Request(f"http://127.0.0.1:{port}/cql", data=json.dumps({"query": "CREATE (n:Release {answer: 42}) RETURN n.answer AS answer"}).encode(),
                          headers={"Authorization": "Bearer local-smoke-fixture", "Content-Type": "application/json"})
        with urlopen(request, timeout=5) as response:
            result = json.load(response)
            assert result == {"ok": True, "count": 1, "rows": [{"answer": 42}]}, result
            print("Built server: /cql returned answer=42")
    finally:
        server.terminate()
        server.wait(timeout=10)
