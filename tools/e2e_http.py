"""HTTP fixtures that use their bound numeric address without reverse DNS."""
from http.server import ThreadingHTTPServer as _ThreadingHTTPServer
from socketserver import TCPServer


class ThreadingHTTPServer(_ThreadingHTTPServer):
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]
