#!/usr/bin/env python3
"""A minimal language server that answers textDocument/codeLens (and codeLens/resolve).

The first line of the document gets a lens that already has its command ("Run"), the second one
a lens that has to be resolved ("2 references"). Everything it is asked is appended to the file
given as the first argument, so tests can see which requests were (not) made.
"""
import json
import sys

log_path = sys.argv[1]


def log(entry):
    with open(log_path, "a") as log_file:
        log_file.write(entry + "\n")


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


def lens_range(line):
    return {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 2}}


while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "capabilities": {
                "textDocumentSync": 1,
                "codeLensProvider": {"resolveProvider": True},
                "executeCommandProvider": {"commands": ["fake.run"]},
            }}})
    elif method == "textDocument/codeLens":
        log("codeLens")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"range": lens_range(0), "command": {"title": "Run", "command": "fake.run"}},
            {"range": lens_range(1), "data": {"what": "references"}},
        ]})
    elif method == "codeLens/resolve":
        log("resolve")
        lens = msg["params"]
        lens["command"] = {"title": "2 references", "command": "fake.show"}
        send({"jsonrpc": "2.0", "id": msg["id"], "result": lens})
    elif method == "workspace/executeCommand":
        log("execute:" + msg["params"]["command"])
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
