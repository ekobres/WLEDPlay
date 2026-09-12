#!/usr/bin/env python3
"""Generate raw PCM (S16LE) for wledplay stdin testing."""
import argparse
import math
import struct


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("-o", "--output", default="-", help="Output file or - for stdout")
    p.add_argument("--rate", type=int, default=44100)
    p.add_argument("--seconds", type=float, default=5.0)
    p.add_argument("--freq", type=float, default=440.0)
    p.add_argument("--channels", type=int, default=2)
    p.add_argument("--amplitude", type=float, default=8000.0)
    args = p.parse_args()

    n = int(args.rate * args.seconds)
    out = open(args.output, "wb") if args.output != "-" else __import__("sys").stdout.buffer
    for i in range(n):
        s = int(args.amplitude * math.sin(2 * math.pi * args.freq * i / args.rate))
        s = max(-32768, min(32767, s))
        for _ in range(args.channels):
            out.write(struct.pack("<h", s))
    if args.output != "-":
        out.close()


if __name__ == "__main__":
    main()
