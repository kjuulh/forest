#!/usr/bin/env python3
"""Tiny webhook test server that receives and prints Forage webhook notifications.

Usage:
    python3 tools/webhook-test-server.py

Then create a webhook integration in Forage pointing to:
    http://localhost:9876/webhook
"""

import json
import hmac
import hashlib
from datetime import datetime, timezone
from http.server import HTTPServer, BaseHTTPRequestHandler


class WebhookHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)

        now = datetime.now(timezone.utc).strftime("%H:%M:%S")
        print(f"\n{'━' * 60}")
        print(f"  [{now}] Webhook received on {self.path}")

        # Print signature if present
        sig = self.headers.get("X-Forest-Signature")
        if sig:
            print(f"  Signature: {sig}")

            # Verify against known test secret if set
            secret = "test-secret"
            expected = "sha256=" + hmac.new(
                secret.encode(), body, hashlib.sha256
            ).hexdigest()
            if sig == expected:
                print(f"  ✓ Signature verified (secret: '{secret}')")
            else:
                print(f"  ✗ Signature mismatch (tried secret: '{secret}')")

        ua = self.headers.get("User-Agent", "")
        if ua:
            print(f"  User-Agent: {ua}")

        # Parse and pretty-print JSON
        try:
            data = json.loads(body)
            event = data.get("event", "unknown")
            org = data.get("organisation", "")
            title = data.get("title", "")
            body_text = data.get("body", "")

            print(f"  Event: {event}")
            print(f"  Org:   {org}")
            print(f"  Title: {title}")
            if body_text:
                print(f"  Body:  {body_text}")

            release = data.get("release")
            if release:
                print(f"  Release:")
                for key in ["destination", "commit_sha", "commit_branch", "source_username", "error_message"]:
                    val = release.get(key)
                    if val:
                        print(f"    {key}: {val}")

            print(f"\n  Full JSON:")
            for line in json.dumps(data, indent=2).split("\n"):
                print(f"  {line}")

        except json.JSONDecodeError:
            print(f"  Raw body: {body.decode('utf-8', errors='replace')}")

        print(f"{'━' * 60}\n")

        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"OK")

    def log_message(self, format, *args):
        pass  # Suppress default access logs


if __name__ == "__main__":
    port = 9876
    server = HTTPServer(("0.0.0.0", port), WebhookHandler)
    print(f"🔔 Webhook test server listening on http://localhost:{port}/webhook")
    print(f"   Configure your Forage webhook URL to: http://localhost:{port}/webhook")
    print(f"   Waiting for notifications...\n")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down.")
        server.server_close()
