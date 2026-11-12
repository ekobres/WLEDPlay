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
import re
import socket
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import ip_address, ip_network
from urllib.parse import parse_qs, urlparse

LISTEN = os.environ.get("AUTOJOIN_LISTEN", "127.0.0.1:8091")
STORAGE = os.environ.get("PYATV_STORAGE", "/host-config/pyatv")
RECEIVER_NAME = os.environ.get("WLEDPLAY_AIRPLAY_NAME", "WLEDPlay")
AIRPLAY_IFACE = os.environ.get("WLEDPLAY_AIRPLAY_INTERFACE", "").strip()
NQPTP_HOST = os.environ.get("NQPTP_CONTROL_HOST", "127.0.0.1")
NQPTP_PORT = int(os.environ.get("NQPTP_CONTROL_PORT", "9000"))

_lock = threading.Lock()
_cache = {
    "self_id": "",
    "discovery_active": False,
    "error": "",
    "devices": [],
}
_probe = {"id": "", "name": RECEIVER_NAME}
_index = {"devices": {}, "self_deviceid": "", "self_name": ""}


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


def nqptp_arm(ip: str) -> bool:
    """Ask stock NQPTP to follow this address and mark the clock active.

    Shairport sends the same datagrams when audio setup starts. Group setup
    needs them earlier, or the leader never opens the audio stream. A bad
    address or a missing NQPTP must not fail the group add.
    """
    text = (ip or "").strip().strip("[]")
    if not text:
        return False
    try:
        ip_address(text)
    except ValueError:
        return False
    timing = f"/nqptp T {text}\0".encode()
    play = b"/nqptp B\0"
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            sock.sendto(timing, (NQPTP_HOST, NQPTP_PORT))
            sock.sendto(play, (NQPTP_HOST, NQPTP_PORT))
        finally:
            sock.close()
    except OSError as exc:
        print(f"Auto Join NQPTP arm failed: {exc}", file=sys.stderr, flush=True)
        return False
    print(f"Auto Join NQPTP following {text}", file=sys.stderr, flush=True)
    return True


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


def _text(value) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", "replace")
    return str(value)


def service_properties(conf) -> dict[str, str]:
    try:
        from pyatv.const import Protocol
    except Exception:
        return {}
    airplay = conf.get_service(Protocol.AirPlay) if conf is not None else None
    if airplay is None:
        return {}
    raw = getattr(airplay, "properties", None) or {}
    try:
        items = list(raw.items())
    except Exception:
        return {}
    return {_text(key).lower(): _text(value).strip() for key, value in items}


def normalize_id(value: str) -> str:
    return re.sub(r"[^0-9a-f]", "", (value or "").lower())


def ids_match(left: str, right: str) -> bool:
    if not left or not right:
        return False
    if left.lower() == right.lower():
        return True
    left_norm, right_norm = normalize_id(left), normalize_id(right)
    return bool(left_norm) and left_norm == right_norm


def mac_device_id(deviceid: str) -> str:
    """AirPlay deviceid as AA:BB:CC:DD:EE:FF. Other identifiers pass through."""
    hexonly = re.sub(r"[^0-9a-fA-F]", "", deviceid or "")
    if len(hexonly) != 12:
        return (deviceid or "").strip()
    return ":".join(hexonly[i : i + 2].upper() for i in range(0, 12, 2))


def receiver_output_ids(deviceid: str, fallback: str) -> list[str]:
    """Ids to give add_output_devices. Shairport is the MAC, not the pairing UUID."""
    found: list[str] = []
    mac = mac_device_id(deviceid)
    if mac:
        found.append(mac)
    if fallback and not any(ids_match(fallback, existing) for existing in found):
        found.append(fallback)
    return found


def _id_set(props: dict, fields: tuple[str, ...], skip: set[str] | None = None) -> set[str]:
    skipped = skip or set()
    found = set()
    for field in fields:
        value = (props.get(field) or "").strip().lower()
        if value and value not in {"0", "null"} and value not in skipped:
            found.add(value)
    return found


def home_ids(props: dict) -> set[str]:
    return _id_set(props, ("pi", "psi"))


def advertised_ids(props: dict) -> set[str]:
    return _id_set(props, ("gid", "pgid", "pi", "psi"))


def session_ids(props: dict) -> set[str]:
    """Group ids that differ from this speaker's own idle identity."""
    return _id_set(props, ("gid", "pgid"), home_ids(props))


def shares_session(member: dict, leader: dict) -> bool:
    session = session_ids(member)
    if not session:
        return False
    return bool(session & advertised_ids(leader))


def receiving_audio(props: dict) -> bool:
    """True when AirPlay status flags show an active audio session.

    An idle Apple TV advertises flags 0x10644. While it is receiving audio
    the same record is 0x30e44, which adds 0x800 and 0x20000.
    """
    raw = (props.get("flags") or props.get("sf") or "").strip()
    if not raw:
        return False
    try:
        flags = int(raw, 0)
    except ValueError:
        return False
    return (flags & 0x800) != 0 or (flags & 0x20000) != 0


def is_group_leader_txt(props: dict) -> bool:
    return (props.get("igl") or "").strip().lower() in {"1", "true", "yes"}


def remember_index(devices_meta: dict, self_deviceid: str, self_name: str) -> None:
    with _lock:
        _index["devices"] = devices_meta
        _index["self_deviceid"] = self_deviceid
        _index["self_name"] = self_name


def snapshot_index() -> dict:
    with _lock:
        return {
            "devices": dict(_index["devices"]),
            "self_deviceid": _index["self_deviceid"],
            "self_name": _index["self_name"],
        }


def leader_hosts(probe_id: str, index: dict, receiver_name: str) -> list[str]:
    """Target address, then group leaders that share its playback session."""
    devices = index.get("devices") or {}
    mine = devices.get(probe_id) or {}
    mine_props = mine.get("props") or {}
    leaders = []
    others = []
    for ident, rec in devices.items():
        if ident == probe_id:
            continue
        name = rec.get("name") or ""
        if receiver_name and name.lower() == receiver_name.lower():
            continue
        if not shares_session(mine_props, rec.get("props") or {}):
            continue
        addr = rec.get("address") or ""
        if not addr:
            continue
        if is_group_leader_txt(rec.get("props") or {}):
            leaders.append(addr)
        else:
            others.append(addr)
    hosts = []
    target = mine.get("address") or ""
    if target:
        hosts.append(target)
    for addr in leaders + others:
        if addr not in hosts:
            hosts.append(addr)
        if len(hosts) >= 4:
            break
    return hosts


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


async def output_snapshot(rc) -> list[tuple[str, str]]:
    current = getattr(rc, "output_devices", []) or []
    if callable(current):
        current = current()
    if asyncio.iscoroutine(current):
        current = await current
    found = []
    for dev in current or []:
        ident = getattr(dev, "identifier", None) or getattr(dev, "id", None) or ""
        name = getattr(dev, "name", None) or ""
        found.append((str(ident), str(name)))
    return found


def output_lists_receiver(current, receiver_ids: list[str], receiver_name: str) -> bool:
    for ident, name in current:
        if receiver_name and name.lower() == receiver_name.lower():
            return True
        if any(ids_match(ident, rid) for rid in receiver_ids):
            return True
    return False


def group_edit_supported(atv) -> bool:
    """add_output_devices lives on pyatv's audio interface, not remote_control."""
    features = getattr(atv, "features", None)
    if features is not None:
        try:
            from pyatv.const import FeatureName, FeatureState

            state = features.get_feature(FeatureName.AddOutputDevices).state
            return state == FeatureState.Available
        except Exception:
            pass
    audio = getattr(atv, "audio", None)
    return callable(getattr(audio, "add_output_devices", None))


def audio_controls(atv):
    audio = getattr(atv, "audio", None)
    if audio is not None and callable(getattr(audio, "add_output_devices", None)):
        return audio
    remote = getattr(atv, "remote_control", None)
    if remote is not None and callable(getattr(remote, "add_output_devices", None)):
        return remote
    return None


def ranked_ids(target_id: str, props_by_id: dict, names: dict, receiver_name: str) -> list[str]:
    """Group leaders first, then the selected speaker, then other session members."""
    if target_id not in props_by_id:
        return []
    mine = props_by_id.get(target_id, {})
    leaders = []
    others = []
    for ident, peer in props_by_id.items():
        if ident == target_id:
            continue
        if receiver_name and (names.get(ident) or "").lower() == receiver_name.lower():
            continue
        if not shares_session(mine, peer) and not shares_session(peer, mine):
            continue
        if is_group_leader_txt(peer):
            leaders.append(ident)
        else:
            others.append(ident)
    ordered = list(leaders)
    ordered.append(target_id)
    for ident in others[:2]:
        if ident not in ordered:
            ordered.append(ident)
    return ordered


async def add_receiver(
    conf, receiver_ids: list[str], receiver_name: str, storage, leader_address: str
) -> tuple[bool, str]:
    """Ask the group leader to add WLEDPlay. Success requires it to show up."""
    nqptp_arm(leader_address)
    atv = await connect_device(conf, storage)
    try:
        if not group_edit_supported(atv):
            return False, "group modification not supported"
        audio = audio_controls(atv)
        add = getattr(audio, "add_output_devices", None)
        if audio is None or not callable(add):
            return False, "group modification not supported"
        current = await output_snapshot(audio)
        if output_lists_receiver(current, receiver_ids, receiver_name):
            return True, "already in group"
        last = "group leader did not add WLEDPlay"
        for rid in receiver_ids:
            try:
                await add(rid)
            except Exception as exc:
                text = str(exc).lower()
                if "not supported" in text or "unsupported" in text:
                    return False, "group modification not supported"
                last = str(exc)
                continue
            current = await output_snapshot(audio)
            if output_lists_receiver(current, receiver_ids, receiver_name):
                return True, ""
        return False, last
    finally:
        await _close(atv)


async def member_is_playing(probe_id, self_id, receiver_name, by_id, props_by_id, names, storage) -> bool:
    """A grouped speaker is playing when its group leader is playing."""
    mine = props_by_id.get(probe_id, {})
    if not session_ids(mine):
        return False
    leaders = []
    others = []
    for ident, conf in by_id.items():
        if ident in {probe_id, self_id}:
            continue
        if receiver_name and (names.get(ident) or "").lower() == receiver_name.lower():
            continue
        peer = props_by_id.get(ident, {})
        if not shares_session(mine, peer):
            continue
        if is_group_leader_txt(peer):
            leaders.append(conf)
        else:
            others.append(conf)
    for conf in (leaders + others)[:3]:
        try:
            if await probe_playing(conf, storage):
                return True
        except Exception:
            continue
    return False


async def scan_hosts(storage, hosts: list[str]):
    import pyatv

    cleaned = [host for host in hosts if host]
    if not cleaned:
        return await scan_devices(storage)
    loop = asyncio.get_running_loop()
    try:
        return await pyatv.scan(loop, timeout=2, hosts=cleaned, storage=storage)
    except TypeError:
        try:
            return await pyatv.scan(timeout=2, hosts=cleaned, storage=storage)
        except TypeError:
            return await scan_devices(storage)


def collect_confs(confs, receiver_name: str, network):
    """Index on-network AirPlay devices. WLEDPlay stays out of the target list."""
    devices = []
    self_id = ""
    self_deviceid = ""
    by_id = {}
    props_by_id = {}
    names = {}
    meta = {}
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
        props = service_properties(conf)
        by_id[rec["id"]] = conf
        props_by_id[rec["id"]] = props
        names[rec["id"]] = rec["name"]
        meta[rec["id"]] = {"address": rec["address"], "name": rec["name"], "props": props}
        if receiver_name and rec["name"].lower() == receiver_name.lower():
            self_id = rec["id"]
            self_deviceid = props.get("deviceid", "")
            continue
        devices.append(rec)
    return devices, self_id, self_deviceid, by_id, props_by_id, names, meta


async def scan_once(loop_storage, probe_id: str, receiver_name: str, network) -> dict:
    storage = loop_storage
    confs = await scan_devices(storage)
    devices, self_id, self_deviceid, by_id, props_by_id, names, meta = collect_confs(
        confs, receiver_name, network
    )
    remember_index(meta, self_deviceid, receiver_name)
    error = ""
    if probe_id and probe_id in by_id:
        try:
            playing = await probe_playing(by_id[probe_id], storage)
        except Exception as exc:
            playing = False
            error = str(exc)
        if not playing:
            try:
                if await member_is_playing(
                    probe_id, self_id, receiver_name, by_id, props_by_id, names, storage
                ):
                    playing = True
                    error = ""
            except Exception:
                playing = False
        if not playing and receiving_audio(props_by_id.get(probe_id, {})):
            playing = True
            error = ""
        for rec in devices:
            if rec["id"] == probe_id:
                rec["playing"] = playing
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
    index = snapshot_index()
    with _lock:
        receiver_name = _probe.get("name") or _index.get("self_name") or RECEIVER_NAME

    async def _run():
        storage = _storage(asyncio.get_running_loop())
        if await _load_storage(storage):
            storage = None
        hosts = leader_hosts(target_id, index, receiver_name)
        confs = await scan_hosts(storage, hosts)
        _devices, _self_id, self_deviceid, by_id, props_by_id, _names, _meta = collect_confs(
            confs, receiver_name, network
        )
        if target_id not in by_id:
            confs = await scan_devices(storage)
            _devices, _self_id, scanned_id, by_id, props_by_id, _names, _meta = collect_confs(
                confs, receiver_name, network
            )
            if scanned_id:
                self_deviceid = scanned_id
        if not self_deviceid:
            self_deviceid = index.get("self_deviceid") or ""
        output_ids = receiver_output_ids(self_deviceid, receiver_id)
        if not output_ids:
            return False, "unable to identify WLEDPlay receiver"
        if target_id not in by_id:
            return False, "unable to identify group leader"
        last = "target is not playing"
        tried = False
        for ident in ranked_ids(target_id, props_by_id, _names, receiver_name):
            conf = by_id.get(ident)
            if conf is None:
                continue
            try:
                playing = await probe_playing(conf, storage)
            except Exception as exc:
                if ident == target_id and not tried:
                    last = str(exc)
                continue
            if not playing and not (
                ident == target_id and receiving_audio(props_by_id.get(target_id, {}))
            ):
                continue
            tried = True
            leader_address = str(getattr(conf, "address", "") or "")
            try:
                ok, err = await add_receiver(
                    conf, output_ids, receiver_name, storage, leader_address
                )
            except Exception as exc:
                text = str(exc).lower()
                if "not supported" in text or "unsupported" in text:
                    last = "group modification not supported"
                    continue
                return False, str(exc)
            if ok:
                return True, err
            if err != "group modification not supported":
                return False, err
            last = err
        if not tried and receiving_audio(props_by_id.get(target_id, {})):
            return False, "group leader is not controllable"
        return False, last

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
