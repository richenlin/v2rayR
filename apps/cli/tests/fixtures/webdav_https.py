import http.server, os, pathlib, ssl, subprocess, tempfile, threading

with tempfile.TemporaryDirectory() as raw:
    path = pathlib.Path(raw)
    cert, key = path / "cert.pem", path / "key.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
                    "-out", str(cert), "-keyout", str(key)], check=True, capture_output=True)
    class WebDAV(http.server.BaseHTTPRequestHandler):
        body = None
        version = 0
        def do_PUT(self):
            if not self.authorized(): return
            cls = type(self)
            conditional = self.headers.get("If-Match")
            if cls.body is not None and conditional != f'"{cls.version}"':
                self.send_error(412); return
            if cls.body is None and self.headers.get("If-None-Match") != "*":
                self.send_error(428); return
            cls.body = self.rfile.read(int(self.headers["Content-Length"]))
            cls.version += 1
            self.send_response(201); self.end_headers()
        def do_GET(self):
            if not self.authorized(): return
            cls = type(self)
            self.send_response(200)
            self.send_header("ETag", f'"{cls.version}"')
            self.send_header("Content-Length", str(len(cls.body)))
            self.end_headers()
            self.wfile.write(cls.body)
        def authorized(self):
            if self.headers.get("Authorization") != "Basic dXNlcjpzZWNyZXQ=":
                self.send_error(401); return False
            return True
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), WebDAV)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(str(cert), str(key))
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {**os.environ, "SSL_CERT_FILE": str(cert), "V2RAYR_WEBDAV_PASSWORD": "secret"}
    url = f"https://127.0.0.1:{server.server_port}/backup.json"
    cli = os.environ["V2RAYR_TEST_CLI"]
    def run(*args, expect=True):
        p = subprocess.run([cli, "--home", str(path/"home"), "backup", *map(str, args)],
                           env=env, text=True, capture_output=True)
        assert (p.returncode == 0) == expect, (p.stdout, p.stderr)
        return p.stdout
    src, dest = path/"source.json", path/"pulled.json"
    run("export", src)
    run("push", src, url, "--username", "user")
    run("push", src, url, "--username", "user", expect=False)
    assert '"etag":"\\"1\\""' in run("pull", url, dest, "--username", "user")
    assert src.read_bytes() == dest.read_bytes()
    run("push", src, url, "--username", "user", "--if-match", '"1"')
    run("push", src, url, "--username", "user", "--if-match", '"1"', expect=False)
    run("import", dest)
    run("import", dest, "--apply")
    server.shutdown()
print("HTTPS WebDAV: authentication, create-only, pull, conditional replace, stale ETag and import passed")
