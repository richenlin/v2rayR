"""Run only in a disposable Linux namespace with the documented helper installed."""
import http.client
import ipaddress
import json
import os
import pathlib
import select
import socketserver
import subprocess
import threading
import time

if os.environ.get("V2RAYR_TUN_ISOLATED_TEST") != "1":
    raise SystemExit("Use a disposable Linux network namespace.")

HELPER = "/usr/local/libexec/v2rayr/v2rayr-helper"
TUN = pathlib.Path("/sys/class/net/v2rayr-tun0")
DEST = "198.51.100.20"


class SocksFixture(socketserver.StreamRequestHandler):
    def handle(self):
        if self.rfile.read(3) != b"\x05\x01\x00":
            return
        self.wfile.write(b"\x05\x00")
        if self.rfile.read(4) != b"\x05\x01\x00\x01":
            return
        assert str(ipaddress.IPv4Address(self.rfile.read(4))) == DEST
        assert int.from_bytes(self.rfile.read(2), "big") == 80
        self.wfile.write(b"\x05\x00\x00\x01" + b"\x00" * 6)
        assert self.rfile.readline() == b"GET /tun-smoke HTTP/1.1\r\n"
        while self.rfile.readline() not in (b"\r\n", b""):
            pass
        self.wfile.write(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")


assert not TUN.exists()
with socketserver.ThreadingTCPServer(("127.0.0.1", 0), SocksFixture) as fixture:
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    command = ["sudo", "-n", "--", HELPER, "tun-stdio"]
    request = {"socks_port": fixture.server_address[1], "upstream": "192.0.2.99"}
    for abrupt in (False, True):
        if abrupt:
            # Kill the controller, leaving the helper/sudo to observe pipe EOF.
            shell = ["python3", "-c", """import subprocess, sys, time
p = subprocess.Popen(sys.argv[2:], stdin=subprocess.PIPE)
p.stdin.write(bytes.fromhex(sys.argv[1]))
p.stdin.flush()
time.sleep(60)
""", (json.dumps(request) + "\n").encode().hex(), *command]
            process = subprocess.Popen(shell, stdout=subprocess.PIPE)
        else:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
            process.stdin.write((json.dumps(request) + "\n").encode())
            process.stdin.flush()
        try:
            assert select.select([process.stdout], [], [], 26)[0], "helper readiness timed out"
            assert process.stdout.readline() == b"ready\n"
            assert TUN.exists()
            request_http = http.client.HTTPConnection(DEST, 80, timeout=5)
            request_http.request("GET", "/tun-smoke")
            assert request_http.getresponse().status == 204
            request_http.close()
        finally:
            if abrupt:
                process.kill()
            else:
                process.stdin.close()
            result = process.wait(timeout=10)
        assert result == (-9 if abrupt else 0)
        deadline = time.monotonic() + 8
        while TUN.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        assert not TUN.exists()
        for family in ("-4", "-6"):
            rules = subprocess.check_output(["ip", family, "-N", "rule", "show"], text=True)
            routes = subprocess.check_output(["ip", family, "-N", "route", "show", "table", "all"], text=True)
            assert "lookup 18622" not in rules
            assert "table 18622" not in routes
print("TUN forwarded traffic through SOCKS and removed routes on EOF and controller SIGKILL")
