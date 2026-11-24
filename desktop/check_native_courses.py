"""Exercise the native webview and its assignment handoff with a loopback fixture."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import subprocess
import tempfile
import time
import threading


class Course(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(b"<title>Robotutor course check</title><script>"
                         b"addEventListener('load',()=>setTimeout(()=>location.href='robotutor://assignment/42',200));"
                         b"</script>")

    def log_message(self, *_):
        pass


state = tempfile.TemporaryDirectory()
try:
    with ThreadingHTTPServer(("127.0.0.1", 0), Course) as service:
        threading.Thread(target=service.serve_forever, daemon=True).start()
        env = dict(os.environ, ROBOTUTOR_WEB_DATA=state.name, ROBOTUTOR_PORT=str(service.server_port))
        proc = subprocess.run([env["ROBOTUTOR_COURSES_BIN"]], capture_output=True, timeout=45, env=env)
        assert proc.returncode == 0, proc.stderr.decode(errors="replace")
        assert proc.stdout.strip() == b"42", proc.stdout
        service.shutdown()
finally:
    # WebView2 child processes can briefly retain cache handles after the window has closed.
    deadline = time.monotonic() + 15
    while True:
        try:
            state.cleanup()
            break
        except PermissionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.2)
print("The native course window loaded the loopback page and returned its assignment to the lesson.")
