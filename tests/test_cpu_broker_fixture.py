import json
import os
import threading
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer

from scripts.testapi_cpu_broker import BrokerHandler


class BrokerFixtureTest(unittest.TestCase):
    def test_contract(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), BrokerHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_port}"
        try:
            with urllib.request.urlopen(base + "/cpu/user_cores?user_id=1") as response:
                self.assertFalse(json.load(response)["is_busy"])
            request = urllib.request.Request(base + "/cpu/acquire", data=b"user_id=1")
            with urllib.request.urlopen(request) as response:
                cores = json.load(response)["cores"]
                self.assertEqual(len(cores), 1)
                self.assertIn(cores[0], os.sched_getaffinity(0))
            request = urllib.request.Request(base + "/cpu/release", data=b"{}")
            with urllib.request.urlopen(request) as response:
                self.assertTrue(json.load(response)["ok"])
            with self.assertRaises(urllib.error.HTTPError) as error:
                urllib.request.urlopen(base + "/unknown")
            self.assertEqual(error.exception.code, 404)
            error.exception.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
