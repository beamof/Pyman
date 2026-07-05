#!/usr/bin/env python3
"""A simple smoke-test script: prints its args and counts to stdout/stderr."""
import sys
import time

print("hello from pyman", flush=True)
print(f"sys.argv = {sys.argv}", flush=True)

for i in range(1, 11):
    stream = sys.stdout if i % 3 else sys.stderr  # every 3rd line -> stderr
    print(f"tick {i}", file=stream, flush=True)
    time.sleep(0.3)

print("done", flush=True)
