#!/usr/bin/env python3
"""Local native-kernel routing fixture; never changes OS proxy or TUN.
First run the routing_benchmark example with an output directory, then pass
that directory here. Set V2RAYR_TEST_XRAY / V2RAYR_TEST_SING_BOX to native cores.
"""
import contextlib
import http.server
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def ready(port, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"kernel exited: {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), .2):
                return
        except OSError:
            time.sleep(.05)
    raise RuntimeError("kernel readiness timed out")


def request(port, host, destination_port):
    with socket.create_connection(("127.0.0.1", port), 3) as sock:
        sock.settimeout(3)
        sock.sendall(f"GET http://{host}:{destination_port}/ HTTP/1.1\r\nHost: {host}:{destination_port}\r\nConnection: close\r\n\r\n".encode())
        result = bytearray()
        while True:
            try:
                data = sock.recv(65536)
            except (ConnectionResetError, TimeoutError):
                break
            if not data:
                break
            result.extend(data)
        return bytes(result)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = self.server.marker
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def main():
    fixtures = pathlib.Path(sys.argv[1])
    xray = os.environ["V2RAYR_TEST_XRAY"]
    cores = {"xray": xray, "sing-box": os.environ["V2RAYR_TEST_SING_BOX"]}
    with contextlib.ExitStack() as stack:
        backends = []
        for marker in [b"DIRECT_PATH", b"PROXY_PATH"]:
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server.marker = marker
            threading.Thread(target=server.serve_forever, daemon=True).start()
            stack.callback(server.server_close)
            stack.callback(server.shutdown)
            backends.append(server.server_port)
        direct_port, proxy_port = backends
        temp = pathlib.Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="v2rayr-routing-")))
        upstream_port = free_port()
        server_config = {
            "inbounds": [{"listen": "127.0.0.1", "port": upstream_port, "protocol": "vless", "settings": {"clients": [{"id": "00000000-0000-0000-0000-000000000000"}], "decryption": "none"}}],
            "outbounds": [{"protocol": "freedom", "settings": {"redirect": f"127.0.0.1:{proxy_port}", "finalRules": [{"action": "allow", "ip": ["127.0.0.1/32"], "port": proxy_port}]}}],
        }
        path = temp / "server.json"
        path.write_text(json.dumps(server_config))
        log = stack.enter_context(open(temp / "server.log", "w+"))
        server = subprocess.Popen([xray, "run", "-c", str(path)], stdout=log, stderr=log, cwd=temp)
        stack.callback(server.wait)
        stack.callback(server.terminate)
        ready(upstream_port, server)
        for core, binary in cores.items():
            config = json.loads((fixtures / f"{core}-flow.json").read_text())
            inbound_port, api_port = free_port(), free_port()
            if core == "xray":
                for inbound in config["inbounds"]:
                    inbound["port"] = free_port()
                    if inbound["protocol"] == "http":
                        inbound["port"] = inbound_port
                config["metrics"]["listen"] = f"127.0.0.1:{api_port}"
                for outbound in config["outbounds"]:
                    if outbound["tag"] == "proxy":
                        outbound["settings"]["vnext"][0]["port"] = upstream_port
            else:
                config["inbounds"][0]["listen_port"] = inbound_port
                config["experimental"]["clash_api"]["external_controller"] = f"127.0.0.1:{api_port}"
                for outbound in config["outbounds"]:
                    if outbound["tag"] == "proxy":
                        outbound["server_port"] = upstream_port
            path = temp / f"{core}.json"
            path.write_text(json.dumps(config))
            with open(temp / f"{core}.log", "w+") as log:
                process = subprocess.Popen([binary, "run", "-c", str(path)], stdout=log, stderr=log, cwd=temp)
                try:
                    ready(inbound_port, process)
                    assert b"DIRECT_PATH" in request(inbound_port, "localhost", direct_port), "custom direct must precede PAC proxy"
                    assert b"PROXY_PATH" in request(inbound_port, "a.example.com", direct_port), "PAC suffix must proxy"
                    blocked = request(inbound_port, "blocked.example.com", direct_port)
                    assert b"DIRECT_PATH" not in blocked and b"PROXY_PATH" not in blocked, "custom block must precede PAC proxy"
                    assert b"DIRECT_PATH" in request(inbound_port, "127.0.0.1", direct_port), "unmatched must direct"
                    print(f"PASS {core}: custom direct, PAC proxy, custom block, unmatched direct")
                except BaseException:
                    log.seek(0)
                    print(log.read(), file=sys.stderr)
                    raise
                finally:
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    main()
