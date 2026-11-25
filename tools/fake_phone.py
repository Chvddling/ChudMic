"""Test the PC app without a phone: connects like the Android app and sends a 440 Hz tone.

Usage:  python fake_phone.py [pc-ip]      (default 127.0.0.1)
Stop with Ctrl+C.
"""
import math
import socket
import struct
import sys
import time

PC = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
PORT = 47801
RATE, FRAME = 48000, 480

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(0.5)
for _ in range(8):
    s.sendto(b"CMH\x01Fake phone", (PC, PORT))
    try:
        data, _ = s.recvfrom(2048)
        if data[:3] == b"CMW":
            print("Connected to", data[4:].decode(errors="replace"))
            break
        if data[:3] == b"CMB":
            sys.exit("PC is busy with another phone")
    except socket.timeout:
        pass
else:
    sys.exit("No answer from PC")

s.setblocking(False)
seq, phase, last_ping = 0, 0.0, 0.0
start = time.perf_counter()
try:
    while True:
        samples = []
        for _ in range(FRAME):
            samples.append(int(8000 * math.sin(phase)))
            phase += 2 * math.pi * 440 / RATE
        s.sendto(b"CMA" + struct.pack("<IB", seq, 0) + struct.pack(f"<{FRAME}h", *samples), (PC, PORT))
        seq += 1
        now = time.perf_counter()
        if now - last_ping > 0.5:
            s.sendto(b"CMP\x00", (PC, PORT))
            last_ping = now
        try:
            data, _ = s.recvfrom(2048)
            if data[:3] == b"CMX":
                print("PC disconnected")
                break
        except OSError:  # nothing received (or Windows "connection reset")
            pass
        # pace at real time (10 ms per packet)
        delay = start + seq * FRAME / RATE - time.perf_counter()
        if delay > 0:
            time.sleep(delay)
except KeyboardInterrupt:
    s.sendto(b"CMX", (PC, PORT))
    print("Bye")
