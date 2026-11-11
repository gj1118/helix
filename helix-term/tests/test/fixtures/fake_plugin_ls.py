#!/usr/bin/env python3
"""A minimal language server for plugin requests: hover answers with the position asked
about, `$/echo` with its params, notifications are written to the log (argv[1])."""
import json
import sys

LOG = sys.argv[1]


def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "capabilities": {"textDocumentSync": 1, "hoverProvider": True}}})
    elif method == "textDocument/hover":
        pos = msg["params"]["position"]
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "contents": "hover at %d:%d" % (pos["line"], pos["character"])}})
    elif method == "$/echo":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": msg.get("params")})
    elif method == "$/fail":
        send({"jsonrpc": "2.0", "id": msg["id"],
              "error": {"code": -32000, "message": "failed on purpose"}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method and method.startswith("$/note"):
        with open(LOG, "a") as log:
            log.write(json.dumps(msg.get("params")) + "\n")
