#!/usr/bin/env python3
"""A scripted debug adapter for the integration tests.

It speaks DAP over stdio. A launch request may carry:

  program   path of the debugged "program" (used as the source of every frame)
  stops     list of (one-based) lines the program stops at, in order
  label     name of the session, echoed in the `process` event

Every request is appended to the file named by the first command line argument (if given) as one JSON line, so
tests can check what the editor asked for.
"""
import json
import os
import sys
import threading

TOTAL_FRAMES = 120
ARRAY_LEN = 250

stdin = sys.stdin.buffer
stdout = sys.stdout.buffer
lock = threading.Lock()
seq = 0
state = {
    "program": "",
    "stops": [3],
    "stop_index": 0,
    "label": "mock",
    "breakpoints": {},
    "next_bp": 1,
}


def log(entry):
    path = sys.argv[1] if len(sys.argv) > 1 else None
    if path:
        with open(path, "a") as f:
            f.write(json.dumps(entry) + "\n")


def send(message):
    global seq
    with lock:
        seq += 1
        message["seq"] = seq
        data = json.dumps(message).encode()
        stdout.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
        stdout.flush()


def respond(request, body=None, success=True, message=None):
    response = {
        "type": "response",
        "request_seq": request["seq"],
        "command": request["command"],
        "success": success,
    }
    if body is not None:
        response["body"] = body
    if message is not None:
        response["message"] = message
    send(response)


def event(name, body=None):
    message = {"type": "event", "event": name}
    if body is not None:
        message["body"] = body
    send(message)


def stop(reason="breakpoint"):
    event(
        "stopped",
        {
            "reason": reason,
            "threadId": 1,
            "allThreadsStopped": True,
            "description": "mock stop",
        },
    )


def current_line():
    stops = state["stops"]
    return stops[min(state["stop_index"], len(stops) - 1)]


def source_of(frame):
    program = state["program"]
    if frame == 2:
        # generated code that only the adapter can provide
        return {"name": "generated.txt", "sourceReference": 7}
    return {"name": os.path.basename(program), "path": program}


def stack_frames(start, levels):
    end = TOTAL_FRAMES if not levels else min(TOTAL_FRAMES, start + levels)
    frames = []
    for i in range(start, end):
        line = current_line() if i == 0 else 1 + (i % 4)
        if i == 2:
            line = 2
        frames.append(
            {
                "id": i + 1,
                "name": "frame%d" % i,
                "source": source_of(i),
                "line": line,
                "column": 1,
            }
        )
    return frames


def variables_for(reference, start, count, flt):
    if reference % 10 == 1:  # locals of a frame
        return [
            {"name": "answer", "value": "42", "type": "int", "variablesReference": 0},
            {"name": "name", "value": '"mock"', "type": "string", "variablesReference": 0},
            {
                "name": "obj",
                "value": "{x, y}",
                "type": "Point",
                "variablesReference": 3,
                "namedVariables": 2,
            },
            {
                "name": "items",
                "value": "Array(%d)" % ARRAY_LEN,
                "type": "Array",
                "variablesReference": 4,
                "indexedVariables": ARRAY_LEN,
            },
        ]
    if reference % 10 == 2:  # globals
        return [{"name": "version", "value": '"1.0"', "type": "string", "variablesReference": 0}]
    if reference == 3:
        return [
            {"name": "x", "value": "1", "type": "int", "variablesReference": 0},
            {"name": "y", "value": "2", "type": "int", "variablesReference": 0},
        ]
    if reference == 4:
        if flt == "named":
            return []
        start = start or 0
        count = count or ARRAY_LEN
        return [
            {"name": "[%d]" % i, "value": str(i * 10), "type": "int", "variablesReference": 0}
            for i in range(start, min(ARRAY_LEN, start + count))
        ]
    return []


def handle(request):
    command = request["command"]
    args = request.get("arguments") or {}
    log({"command": command, "arguments": args})

    if command == "initialize":
        respond(
            request,
            {
                "supportsConfigurationDoneRequest": True,
                "supportsConditionalBreakpoints": True,
                "supportsLogPoints": True,
                "supportsEvaluateForHovers": True,
            },
        )
    elif command in ("launch", "attach"):
        state["program"] = args.get("program", "")
        state["stops"] = args.get("stops", [3])
        state["label"] = args.get("label", "mock")
        respond(request)
        event("initialized")
    elif command == "setBreakpoints":
        source = args.get("source", {})
        key = source.get("path") or "ref:%s" % source.get("sourceReference")
        breakpoints = []
        for bp in args.get("breakpoints", []):
            breakpoints.append(
                {"id": state["next_bp"], "verified": True, "line": bp["line"]}
            )
            state["next_bp"] += 1
        state["breakpoints"][key] = breakpoints
        respond(request, {"breakpoints": breakpoints})
    elif command == "setExceptionBreakpoints":
        respond(request)
    elif command == "configurationDone":
        respond(request)
        stop()
    elif command == "threads":
        respond(request, {"threads": [{"id": 1, "name": "main"}]})
    elif command == "stackTrace":
        start = args.get("startFrame") or 0
        levels = args.get("levels") or 0
        respond(
            request,
            {"stackFrames": stack_frames(start, levels), "totalFrames": TOTAL_FRAMES},
        )
    elif command == "scopes":
        frame = args["frameId"]
        respond(
            request,
            {
                "scopes": [
                    {"name": "Locals", "variablesReference": frame * 10 + 1, "expensive": False},
                    {"name": "Globals", "variablesReference": frame * 10 + 2, "expensive": True},
                ]
            },
        )
    elif command == "variables":
        respond(
            request,
            {
                "variables": variables_for(
                    args["variablesReference"],
                    args.get("start"),
                    args.get("count"),
                    args.get("filter"),
                )
            },
        )
    elif command == "evaluate":
        expression = args["expression"]
        if expression == "answer":
            respond(request, {"result": "42", "type": "int", "variablesReference": 0})
        elif expression == "obj":
            respond(
                request,
                {"result": "{x, y}", "type": "Point", "variablesReference": 3, "namedVariables": 2},
            )
        elif expression == "boom":
            respond(request, success=False, message="cannot evaluate 'boom'")
        else:
            respond(request, {"result": "<%s>" % expression, "variablesReference": 0})
    elif command == "source":
        reference = args.get("sourceReference") or args.get("source", {}).get("sourceReference")
        respond(request, {"content": "generated line 1\ngenerated line 2\ngenerated line 3\n"})
    elif command in ("next", "stepIn", "stepOut"):
        state["stop_index"] += 1
        respond(request)
        stop("step")
    elif command == "continue":
        respond(request, {"allThreadsContinued": True})
        event("continued", {"threadId": 1, "allThreadsContinued": True})
        state["stop_index"] += 1
        if state["stop_index"] < len(state["stops"]):
            stop()
        else:
            event("exited", {"exitCode": 0})
            event("terminated")
    elif command == "pause":
        respond(request)
        stop("pause")
    elif command in ("disconnect", "terminate"):
        respond(request)
        if command == "disconnect":
            sys.exit(0)
    else:
        respond(request, success=False, message="unsupported: %s" % command)


def main():
    while True:
        headers = {}
        while True:
            line = stdin.readline()
            if not line:
                return
            line = line.strip()
            if not line:
                break
            name, _, value = line.decode().partition(":")
            headers[name.lower()] = value.strip()
        body = stdin.read(int(headers["content-length"]))
        message = json.loads(body)
        if message.get("type") == "request":
            handle(message)


if __name__ == "__main__":
    main()
