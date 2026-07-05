#!/usr/bin/env python3
"""An interactive script: reads lines from stdin and echoes them back.

Used to demonstrate PyMan's input feature — type in the bottom input box,
press Enter, and your line is sent to this script's stdin and echoed back
to the log. Type `quit` to exit (the script also exits cleanly on EOF, which
happens when you stop the task).
"""
import sys

print("echo.py ready — type a line and press Enter (or 'quit' to exit)", flush=True)

while True:
    try:
        line = sys.stdin.readline()
    except (KeyboardInterrupt, EOFError):
        break
    if not line:
        # EOF (stdin closed): exit cleanly.
        break
    line = line.rstrip("\r\n")
    print(f"echo: {line}", flush=True)
    if line.strip().lower() == "quit":
        break

print("echo.py done", flush=True)
