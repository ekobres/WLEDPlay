#!/usr/bin/env python3
"""Verify WLED receives Audio Sync UDP packets (diagnoses VLAN/firewall issues)."""
import json
import struct
import sys
import time
import urllib.request

WLED_IP = sys.argv[1] if len(sys.argv) > 1 else "10.11.1.124"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 11988


def wled_audio_source() -> list:
    with urllib.request.urlopen(f"http://{WLED_IP}/json/info", timeout=3) as r:
        data = json.load(r)
    return data.get("u", {}).get("Audio Source", [])


def make_packet() -> bytes:
    fft = [255] * 16
    return struct.pack(
        "<6s2B2fBB16B2B2f",
        b"00002",
        0,
        0,
        200.0,
        200.0,
        1,
        0,
        *fft,
        0,
        0,
        5000.0,
        440.0,
    )


def main() -> int:
    import socket

    print(f"WLED Audio Source before: {wled_audio_source()}")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 20)

    for i in range(150):
        pkt = make_packet()
        sock.sendto(pkt, (WLED_IP, PORT))
        sock.sendto(pkt, ("239.0.0.1", PORT))
        time.sleep(0.02)

    time.sleep(0.3)
    after = wled_audio_source()
    print(f"WLED Audio Source after:  {after}")

    text = " ".join(after)
    if "receiving" in text or " v2" in text:
        print("SUCCESS: WLED is receiving Audio Sync packets.")
        return 0

    print("FAIL: WLED still idle — UDP packets are not reaching the device.")
    print()
    print("Common cause: your computer and WLED are on different VLANs/subnets.")
    print("HTTP to WLED may work while UDP 11988 is blocked by firewall rules.")
    print()
    print("Fix options:")
    print("  1. Run WLEDPlay on a host in the SAME VLAN as WLED (e.g. IoT VLAN 10.11.x)")
    print("  2. Add a firewall rule allowing UDP port 11988 to the WLED IP")
    print("  3. Avoid cross-VLAN multicast; use unicast to the WLED IP from same VLAN")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
