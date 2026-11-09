#!/usr/bin/env python3
"""A minimal language server serving data that is cached in the document.

With "full" as the second argument it answers textDocument/documentLink (a link on the first
word, to file:///full), textDocument/documentColor (a red swatch on the first word) and
textDocument/codeLens (a "Run full" lens on the first line). With "links" it only answers
textDocument/documentLink, linking to file:///links. Every request is appended to the file
given as the first argument.
"""
import json
import sys

log_path, mode = sys.argv[1], sys.argv[2]


def log(entry):
    with open(log_path, "a") as log_file:
        log_file.write(mode + ":" + entry + "\n")


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


FIRST_WORD = {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}

capabilities = {"textDocumentSync": 1, "documentLinkProvider": {}}
if mode == "full":
    capabilities["colorProvider"] = True
    capabilities["codeLensProvider"] = {"resolveProvider": False}

while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": capabilities}})
    elif method == "textDocument/documentLink":
        log("documentLink")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"range": FIRST_WORD, "target": "file:///" + mode},
        ]})
    elif method == "textDocument/documentColor":
        log("documentColor")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"range": FIRST_WORD, "color": {"red": 1, "green": 0, "blue": 0, "alpha": 1}},
        ]})
    elif method == "textDocument/codeLens":
        log("codeLens")
        send({"jsonrpc": "2.0", "id": msg["id"], "result": [
            {"range": FIRST_WORD, "command": {"title": "Run full", "command": "fake.run"}},
        ]})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    elif method == "exit":
        break
    elif "id" in msg:
        send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
