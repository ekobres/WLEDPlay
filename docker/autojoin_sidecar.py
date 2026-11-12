#!/usr/bin/env python3
"""Localhost Auto Join control plane (pyatv).

Binds 127.0.0.1 only. WLEDPlay's operator API remains the only UI.
Credentials stay in a pyatv settings file. PYATV_STORAGE may be that file
or a directory; a directory is stored as <dir>/pyatv.json. FileStorage
cannot open a directory (Errno 21), and that used to abort discovery.

GET  /v1/scan?name=WLEDPlay&probe=<target-id>
POST /v1/join   {"target_id","receiver_id"}
"""

from __future__ import annotations

import asyncio
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import ip_address, ip_network
from urllib.parse import parse_qs, urlparse

LISTEN = os.environ.get("AUTOJOIN_LISTEN", "127.0.0.1:8091")
STORAGE = os.environ.get("PYATV_STORAGE", "/host-config/pyatv")
RECEIVER_NAME = os.environ.get("WLEDPLAY_AIRPLAY_NAME", "WLEDPlay")
AIRPLAY_IFACE = os.environ.get("WLEDPLAY_AIRPLAY_INTERFACE", "").strip()

_lock = threading.Lock()
_cache = {
    "self_id": "",
    "discovery_active": False,
    "error": "",
    "devices": [],
}
_probe = {"id": "", "name": RECEIVER_NAME}


def iface_network(name: str):
    """IPv4 network for a NIC, or None when it cannot be determined."""
    if not name:
        return None
    try:
        import subprocess

        proc = subprocess.run(
            ["ip", "-4", "-o", "addr", "show", "dev", name],
            text=True,
            capture_output=True,
            timeout=2,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    for line in proc.stdout.splitlines():
        parts = line.split()
        if "inet" not in parts:
            continue
        cidr = parts[parts.index("inet") + 1]
        try:
            return ip_network(cidr, strict=False)
        except ValueError:
            continue
    return None


def address_on_network(addr: str, network) -> bool:
    if network is None or not addr:
        return True
    try:
        return ip_address(addr) in network
    except (ValueError, TypeError):
        # IPv6 addresses are not members of the IPv4 interface network.
        # Drop the device; do not abort the rest of the scan.
        return False


def _model_name(conf) -> str:
    info = getattr(conf, "device_info", None)
    model = getattr(info, "model", None) if info else None
    if model is None:
        return ""
    name = getattr(model, "name", None)
    return str(name or model)


def _output_id(conf) -> str:
    info = getattr(conf, "device_info", None)
    if info is None:
        return ""
    for attr in ("output_device_id", "airplay_id"):
        value = getattr(info, attr, None)
        if callable(value):
            try:
                value = value()
            except Exception:
                value = None
        if value:
            return str(value)
    getter = getattr(info, "get", None)
    if callable(getter):
        value = getter("airplay_id") or getter("output_device_id")
        if value:
            return str(value)
    return ""


def device_record(conf) -> dict | None:
    try:
        from pyatv.const import Protocol
    except Exception:
        return None
    airplay = conf.get_service(Protocol.AirPlay)
    if airplay is None:
        return None
    output_id = _output_id(conf)
    ident = str(conf.identifier or "")
    stable = output_id or ident
    if not stable:
        return None
    address = str(getattr(conf, "address", "") or "")
    return {
        "id": stable,
        "name": str(conf.name or ""),
        "model": _model_name(conf),
        "address": address,
        "available": True,
        "playing": False,
        "airplay2": True,
    }


async def _close(atv) -> None:
    close = getattr(atv, "close", None)
    if close is None:
        return
    result = close()
    if asyncio.iscoroutine(result):
        await result


async def scan_devices(storage):
    """pyatv 0.16 requires the running loop; newer releases take keywords only."""
    import pyatv

    loop = asyncio.get_running_loop()
    try:
        return await pyatv.scan(loop, timeout=4, storage=storage)
    except TypeError:
        return await pyatv.scan(timeout=4, storage=storage)


async def connect_device(conf, storage):
    import pyatv

    loop = asyncio.get_running_loop()
    try:
        return await pyatv.connect(conf, loop, storage=storage)
    except TypeError:
        return await pyatv.connect(conf, storage=storage)


async def read_playing(atv) -> bool:
    """pyatv 0.16 exposes playing() as a coroutine method, not a property."""
    from pyatv.const import DeviceState

    playing = atv.metadata.playing
    if callable(playing):
        playing = playing()
    if asyncio.iscoroutine(playing):
        playing = await playing
    state = getattr(playing, "device_state", None)
    return state in (DeviceState.Playing, DeviceState.Loading)


async def probe_playing(conf, storage) -> bool:
    atv = await connect_device(conf, storage)
    try:
        return await read_playing(atv)
    finally:
        await _close(atv)


async def add_receiver(conf, receiver_id: str, storage) -> tuple[bool, str]:
    atv = await connect_device(conf, storage)
    try:
        rc = atv.remote_control
        current = getattr(rc, "output_devices", []) or []
        if asyncio.iscoroutine(current):
            current = await current
        have = []
        for dev in current:
            ident = getattr(dev, "identifier", None) or getattr(dev, "id", None)
            if ident:
                have.append(str(ident))
        if receiver_id in have:
            return True, "already in group"
        await rc.add_output_devices(receiver_id)
        return True, ""
    finally:
        await _close(atv)


async def scan_once(loop_storage, probe_id: str, receiver_name: str, network) -> dict:
    storage = loop_storage
    confs = await scan_devices(storage)
    devices = []
    self_id = ""
    by_id = {}
    for conf in confs:
        rec = device_record(conf)
        if rec is None:
            continue
        if not address_on_network(rec["address"], network):
            continue
        # One row per id. A second address family must not add another option
        # with the same value, or the select jumps to the first match.
        if rec["id"] in by_id:
            continue
        by_id[rec["id"]] = conf
        if receiver_name and rec["name"].lower() == receiver_name.lower():
            self_id = rec["id"]
            continue
        devices.append(rec)
    error = ""
    if probe_id and probe_id in by_id:
        try:
            playing = await probe_playing(by_id[probe_id], storage)
        except Exception as exc:
            playing = False
            error = str(exc)
        for rec in devices:
            if rec["id"] == probe_id:
                rec["playing"] = playing
    elif probe_id:
        error = ""
    return {
        "self_id": self_id,
        "discovery_active": True,
        "error": error,
        "devices": devices,
    }


def storage_file() -> str:
    """FileStorage path. A directory (the Docker default) becomes pyatv.json inside it."""
    raw = STORAGE
    if raw.endswith((".json", ".conf")) and not os.path.isdir(raw):
        parent = os.path.dirname(raw) or "."
        os.makedirs(parent, exist_ok=True)
        return raw
    os.makedirs(raw, exist_ok=True)
    return os.path.join(raw, "pyatv.json")


def _storage(loop):
    try:
        from pyatv.storage.file_storage import FileStorage

        return FileStorage(storage_file(), loop)
    except Exception:
        return None


async def _load_storage(storage) -> str:
    """Load credentials. Failure returns an error string and must not cancel a scan."""
    if storage is None or not hasattr(storage, "load"):
        return ""
    try:
        load = storage.load()
        if asyncio.iscoroutine(load):
            await load
        return ""
    except Exception as exc:
        return str(exc)


def refresh(probe_id: str, receiver_name: str) -> None:
    network = iface_network(AIRPLAY_IFACE)
    try:
        loop = asyncio.new_event_loop()
        try:
            storage = _storage(loop)
            storage_error = loop.run_until_complete(_load_storage(storage))
            if storage_error:
                storage = None
            payload = loop.run_until_complete(
                scan_once(storage, probe_id, receiver_name, network)
            )
            if storage_error and not payload.get("error"):
                payload["error"] = storage_error
        finally:
            loop.close()
    except Exception as exc:
        payload = {
            "self_id": "",
            "discovery_active": False,
            "error": str(exc),
            "devices": [],
        }
    with _lock:
        _cache.update(payload)


def join(target_id: str, receiver_id: str) -> tuple[bool, str]:
    if not target_id or not receiver_id:
        return False, "target_id and receiver_id are required"
    network = iface_network(AIRPLAY_IFACE)

    async def _run():
        storage = _storage(asyncio.get_running_loop())
        if await _load_storage(storage):
            storage = None
        confs = await scan_devices(storage)
        target = None
        for conf in confs:
            rec = device_record(conf)
            if rec and rec["id"] == target_id and address_on_network(rec["address"], network):
                target = conf
                break
        if target is None:
            return False, "unable to identify group leader"
        try:
            return await add_receiver(target, receiver_id, storage)
        except Exception as exc:
            text = str(exc).lower()
            if "not supported" in text or "unsupported" in text:
                return False, "group modification not supported"
            return False, str(exc)

    try:
        return asyncio.run(_run())
    except Exception as exc:
        return False, str(exc)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, fmt: str, *args) -> None:
        return

    def _send(self, code: int, payload: dict) -> None:
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path != "/v1/scan":
            self._send(404, {"ok": False, "error": "not found"})
            return
        qs = parse_qs(parsed.query)
        name = (qs.get("name") or [RECEIVER_NAME])[0]
        probe = (qs.get("probe") or [""])[0]
        with _lock:
            _probe["id"] = probe
            _probe["name"] = name
            snapshot = dict(_cache)
            snapshot["devices"] = list(_cache["devices"])
        self._send(200, snapshot)

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path != "/v1/join":
            self._send(404, {"ok": False, "error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw.decode() or "{}")
        except json.JSONDecodeError:
            self._send(400, {"ok": False, "error": "invalid json"})
            return
        ok, err = join(str(body.get("target_id", "")), str(body.get("receiver_id", "")))
        self._send(200 if ok else 502, {"ok": ok, "error": err})


def _scan_loop() -> None:
    while True:
        with _lock:
            probe = _probe["id"]
            name = _probe["name"] or RECEIVER_NAME
        refresh(probe, name)
        time_sleep()


def time_sleep() -> None:
    import time

    time.sleep(2)


def main() -> None:
    host, _, port = LISTEN.rpartition(":")
    if not host:
        host = "127.0.0.1"
    server = ThreadingHTTPServer((host, int(port)), Handler)
    threading.Thread(target=_scan_loop, name="autojoin-scan", daemon=True).start()
    server.serve_forever()


if __name__ == "__main__":
    main()
