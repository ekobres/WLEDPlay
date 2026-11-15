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
import sys
import threading
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import ip_address, ip_network
from urllib.parse import parse_qs, urlparse

LISTEN = os.environ.get("AUTOJOIN_LISTEN", "127.0.0.1:8091")
STORAGE = os.environ.get("PYATV_STORAGE", "/host-config/pyatv")
RECEIVER_NAME = os.environ.get("WLEDPLAY_AIRPLAY_NAME", "WLEDPlay")
AIRPLAY_IFACE = os.environ.get("WLEDPLAY_AIRPLAY_INTERFACE", "").strip()
GROUP_UNSUPPORTED = "group modification not supported"

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
        "paused": False,
        "airplay2": True,
        "phone_owned": False,
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


def phone_owns_playback(target_id: str, props_by_id: dict) -> bool:
    """True when every speaker we could ask is receiving someone else's audio.

    An origin speaker keeps the receiver-session bits clear. Measured while
    one HomePod owned playback and was grouped: flags 0x198404. A speaker
    taking a stream from somewhere else sets those bits. The sender, a phone,
    iPad, Mac, or Apple TV, is not one of those speakers.
    """
    mine = props_by_id.get(target_id) or {}
    leaders = []
    if is_group_leader_txt(mine):
        leaders.append(mine)
    for ident, peer in props_by_id.items():
        if ident == target_id or not is_group_leader_txt(peer):
            continue
        if shares_session(mine, peer) or shares_session(peer, mine):
            leaders.append(peer)
    candidates = leaders or [mine]
    if any(not receiving_audio(peer) for peer in candidates):
        return False
    return any(receiving_audio(peer) for peer in candidates)


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


def _is_receiver_name(name: str, receiver_name: str) -> bool:
    return bool(receiver_name) and name.lower() == receiver_name.lower()


def _is_session_peer(
    ident: str,
    peer: dict,
    target_id: str,
    mine: dict,
    names: dict,
    receiver_name: str,
    skipped: set[str],
    either_direction: bool,
) -> bool:
    if ident == target_id or ident in skipped:
        return False
    if _is_receiver_name(names.get(ident) or "", receiver_name):
        return False
    if either_direction and shares_session(peer, mine):
        return True
    return shares_session(mine, peer)


def session_peer_ids(
    target_id: str,
    props_by_id: dict,
    names: dict,
    receiver_name: str,
    skipped: set[str],
    either_direction: bool,
) -> tuple[list[str], list[str]]:
    """Leaders, then other members, that share a playback session with the target."""
    mine = props_by_id.get(target_id) or {}
    leaders: list[str] = []
    others: list[str] = []
    for ident, peer in props_by_id.items():
        if not _is_session_peer(
            ident, peer, target_id, mine, names, receiver_name, skipped, either_direction
        ):
            continue
        if is_group_leader_txt(peer):
            leaders.append(ident)
        else:
            others.append(ident)
    return leaders, others


def _append_unique(hosts: list[str], addr: str) -> None:
    if addr and addr not in hosts:
        hosts.append(addr)


def _host_list(devices: dict, probe_id: str, peer_ids: list[str]) -> list[str]:
    """Target address, then peer addresses, capped at four."""
    hosts: list[str] = []
    target = (devices.get(probe_id) or {}).get("address") or ""
    _append_unique(hosts, target)
    for ident in peer_ids:
        if len(hosts) >= 4:
            break
        addr = (devices.get(ident) or {}).get("address") or ""
        _append_unique(hosts, addr)
    return hosts


def session_view(
    probe_id: str, props_by_id: dict, names: dict, receiver_name: str
) -> dict:
    """Selected speaker's advertised flags and the other speakers in its session."""
    mine = props_by_id.get(probe_id) or {}
    target_name = names.get(probe_id) or ""
    leaders, others = session_peer_ids(
        probe_id, props_by_id, names, receiver_name, set(), True
    )
    if is_group_leader_txt(mine):
        leader = target_name
        member_ids = leaders + others
    else:
        leader = names.get(leaders[0], "") if leaders else ""
        member_ids = leaders[1:] + others
    members = []
    for ident in member_ids:
        name = names.get(ident) or ident
        if name and name not in members:
            members.append(name)
    return {
        "target_name": target_name,
        "flags": (mine.get("flags") or mine.get("sf") or ""),
        "igl": mine.get("igl") or "",
        "gcgl": mine.get("gcgl") or "",
        "gid": mine.get("gid") or "",
        "leader": leader,
        "members": members,
    }


def leader_hosts(probe_id: str, index: dict, receiver_name: str) -> list[str]:
    """Target address, then group leaders that share its playback session."""
    devices = index.get("devices") or {}
    props_by_id = {ident: (rec.get("props") or {}) for ident, rec in devices.items()}
    names = {ident: (rec.get("name") or "") for ident, rec in devices.items()}
    leaders, others = session_peer_ids(
        probe_id, props_by_id, names, receiver_name, set(), False
    )
    return _host_list(devices, probe_id, leaders + others)


async def read_playback(atv) -> str:
    """playing, paused, or idle. pyatv 0.16 exposes playing() as a coroutine."""
    from pyatv.const import DeviceState

    playing = atv.metadata.playing
    if callable(playing):
        playing = playing()
    if asyncio.iscoroutine(playing):
        playing = await playing
    state = getattr(playing, "device_state", None)
    if state in (DeviceState.Playing, DeviceState.Loading):
        return "playing"
    if state == DeviceState.Paused:
        return "paused"
    return "idle"


async def probe_playback(conf, storage) -> str:
    atv = await connect_device(conf, storage)
    try:
        return await read_playback(atv)
    finally:
        await _close(atv)


async def probe_playing(conf, storage) -> bool:
    return await probe_playback(conf, storage) == "playing"


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


def _with_target(leaders: list[str], target_id: str, others: list[str]) -> list[str]:
    ordered = [*leaders, target_id]
    for ident in others[:2]:
        if ident not in ordered:
            ordered.append(ident)
    return ordered


def ranked_ids(target_id: str, props_by_id: dict, names: dict, receiver_name: str) -> list[str]:
    """Group leaders first, then the selected speaker, then other session members."""
    if target_id not in props_by_id:
        return []
    leaders, others = session_peer_ids(
        target_id, props_by_id, names, receiver_name, set(), True
    )
    return _with_target(leaders, target_id, others)


async def add_receiver(
    conf, receiver_ids: list[str], receiver_name: str, storage
) -> tuple[bool, str]:
    """Ask the group leader to add WLEDPlay. Success requires it to show up."""
    atv = await connect_device(conf, storage)
    try:
        if not group_edit_supported(atv):
            return False, GROUP_UNSUPPORTED
        audio = audio_controls(atv)
        add = getattr(audio, "add_output_devices", None)
        if audio is None or not callable(add):
            return False, GROUP_UNSUPPORTED
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
                    return False, GROUP_UNSUPPORTED
                last = str(exc)
                continue
            current = await output_snapshot(audio)
            if output_lists_receiver(current, receiver_ids, receiver_name):
                return True, ""
        return False, last
    finally:
        await _close(atv)


async def _playing_quietly(conf, storage) -> bool:
    try:
        return await probe_playing(conf, storage)
    except Exception:
        return False


async def member_is_playing(probe_id, self_id, receiver_name, by_id, props_by_id, names, storage) -> bool:
    """A grouped speaker is playing when its group leader is playing."""
    if not session_ids(props_by_id.get(probe_id, {})):
        return False
    leaders, others = session_peer_ids(
        probe_id, props_by_id, names, receiver_name, {self_id}, False
    )
    for ident in (leaders + others)[:3]:
        conf = by_id.get(ident)
        if conf is not None and await _playing_quietly(conf, storage):
            return True
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


@dataclass
class ScanIndex:
    devices: list
    self_id: str
    self_deviceid: str
    by_id: dict
    props_by_id: dict
    names: dict
    meta: dict


def collect_confs(confs, receiver_name: str, network) -> ScanIndex:
    """Index on-network AirPlay devices. WLEDPlay stays out of the target list."""
    devices = []
    self_id = ""
    self_deviceid = ""
    by_id = {}
    props_by_id = {}
    names = {}
    meta = {}
    for conf in confs:
        recorded = _record_conf(conf, receiver_name, network, by_id)
        if recorded is None:
            continue
        rec, props, is_self = recorded
        by_id[rec["id"]] = conf
        props_by_id[rec["id"]] = props
        names[rec["id"]] = rec["name"]
        meta[rec["id"]] = {"address": rec["address"], "name": rec["name"], "props": props}
        if is_self:
            self_id = rec["id"]
            self_deviceid = props.get("deviceid", "")
            continue
        devices.append(rec)
    return ScanIndex(devices, self_id, self_deviceid, by_id, props_by_id, names, meta)


def _record_conf(conf, receiver_name: str, network, by_id: dict):
    """None when this advertisement is not a new on-network AirPlay device."""
    rec = device_record(conf)
    if rec is None or not address_on_network(rec["address"], network):
        return None
    # One row per id. A second address family must not add another option
    # with the same value, or the select jumps to the first match.
    if rec["id"] in by_id:
        return None
    props = service_properties(conf)
    return rec, props, _is_receiver_name(rec["name"], receiver_name)


def _has_probe(probe_id: str, found: ScanIndex) -> bool:
    return bool(probe_id) and probe_id in found.by_id


async def _playback_result(conf, storage) -> tuple[str, str]:
    try:
        return await probe_playback(conf, storage), ""
    except Exception as exc:
        return "idle", str(exc)


async def _member_playing(probe_id: str, found: ScanIndex, receiver_name: str, storage) -> bool:
    try:
        return await member_is_playing(
            probe_id,
            found.self_id,
            receiver_name,
            found.by_id,
            found.props_by_id,
            found.names,
            storage,
        )
    except Exception:
        return False


async def _playing_without_target(
    probe_id: str, found: ScanIndex, receiver_name: str, storage, error: str
) -> tuple[bool, str]:
    if await _member_playing(probe_id, found, receiver_name, storage):
        return True, ""
    if receiving_audio(found.props_by_id.get(probe_id, {})):
        return True, ""
    return False, error


def _mark_playback(found: ScanIndex, probe_id: str, kind: str) -> None:
    for rec in found.devices:
        rec["phone_owned"] = phone_owns_playback(rec["id"], found.props_by_id)
        if rec["id"] == probe_id:
            rec["playing"] = kind == "playing"
            rec["paused"] = kind == "paused"


async def _mark_probe(probe_id: str, found: ScanIndex, receiver_name: str, storage) -> str:
    kind, error = await _playback_result(found.by_id[probe_id], storage)
    if kind == "idle":
        kind, error = await _idle_or_playing(
            probe_id, found, receiver_name, storage, error
        )
    _mark_playback(found, probe_id, kind)
    return error


async def _idle_or_playing(
    probe_id: str, found: ScanIndex, receiver_name: str, storage, error: str
) -> tuple[str, str]:
    playing, error = await _playing_without_target(
        probe_id, found, receiver_name, storage, error
    )
    if playing:
        return "playing", error
    return "idle", error


async def scan_once(loop_storage, probe_id: str, receiver_name: str, network) -> dict:
    storage = loop_storage
    found = collect_confs(await scan_devices(storage), receiver_name, network)
    remember_index(found.meta, found.self_deviceid, receiver_name)
    error = ""
    if _has_probe(probe_id, found):
        error = await _mark_probe(probe_id, found, receiver_name, storage)
    return {
        "self_id": found.self_id,
        "discovery_active": True,
        "error": error,
        "devices": found.devices,
        "session": session_view(
            probe_id, found.props_by_id, found.names, receiver_name
        ),
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


@dataclass
class _Located:
    found: ScanIndex
    output_ids: list[str]
    receiver_name: str
    storage: object
    error: str = ""


class _Attempt:
    def __init__(self) -> None:
        self.last = "target is not playing"
        self.tried = False

    def note_play_error(self, ident: str, target_id: str, detail: str) -> None:
        if ident == target_id and not self.tried:
            self.last = detail


def _receiver_name() -> str:
    with _lock:
        return _probe.get("name") or _index.get("self_name") or RECEIVER_NAME


async def _usable_storage():
    storage = _storage(asyncio.get_running_loop())
    if await _load_storage(storage):
        return None
    return storage


def _remembered_deviceid(self_deviceid: str, index: dict) -> str:
    return self_deviceid or index.get("self_deviceid") or ""


async def _scan_for_target(storage, target_id: str, receiver_name: str, network, index: dict):
    hosts = leader_hosts(target_id, index, receiver_name)
    found = collect_confs(await scan_hosts(storage, hosts), receiver_name, network)
    self_deviceid = found.self_deviceid
    if target_id in found.by_id:
        return found, _remembered_deviceid(self_deviceid, index)
    wider = collect_confs(await scan_devices(storage), receiver_name, network)
    if wider.self_deviceid:
        self_deviceid = wider.self_deviceid
    return wider, _remembered_deviceid(self_deviceid, index)


def _locate_error(target_id: str, found: ScanIndex, output_ids: list[str]) -> str:
    if not output_ids:
        return "unable to identify WLEDPlay receiver"
    if target_id not in found.by_id:
        return "unable to identify group leader"
    return ""


async def _locate(target_id: str, receiver_id: str) -> _Located:
    network = iface_network(AIRPLAY_IFACE)
    index = snapshot_index()
    receiver_name = _receiver_name()
    storage = await _usable_storage()
    found, self_deviceid = await _scan_for_target(
        storage, target_id, receiver_name, network, index
    )
    output_ids = receiver_output_ids(self_deviceid, receiver_id)
    return _Located(
        found,
        output_ids,
        receiver_name,
        storage,
        _locate_error(target_id, found, output_ids),
    )


def _worth_adding(ident: str, target_id: str, kind: str, props_by_id: dict) -> bool:
    if kind in {"playing", "paused"}:
        return True
    return ident == target_id and receiving_audio(props_by_id.get(target_id, {}))


def _unsupported(exc: Exception) -> bool:
    text = str(exc).lower()
    return "not supported" in text or "unsupported" in text


def _add_exception(state: _Attempt, exc: Exception):
    if _unsupported(exc):
        state.last = GROUP_UNSUPPORTED
        return None
    return False, str(exc)


def _add_status(state: _Attempt, ok: bool, err: str):
    if ok:
        return True, err
    if err != GROUP_UNSUPPORTED:
        return False, err
    state.last = err
    return None


async def _add_outcome(state: _Attempt, conf, located: _Located):
    try:
        ok, err = await add_receiver(
            conf, located.output_ids, located.receiver_name, located.storage
        )
    except Exception as exc:
        return _add_exception(state, exc)
    return _add_status(state, ok, err)


async def _ask_one(state: _Attempt, ident: str, target_id: str, located: _Located):
    conf = located.found.by_id.get(ident)
    if conf is None:
        return None
    kind, play_error = await _playback_result(conf, located.storage)
    if play_error:
        state.note_play_error(ident, target_id, play_error)
        return None
    if not _worth_adding(ident, target_id, kind, located.found.props_by_id):
        return None
    state.tried = True
    return await _add_outcome(state, conf, located)


def _attempt_result(state: _Attempt, props: dict) -> tuple[bool, str]:
    if not state.tried and receiving_audio(props):
        return False, "group leader is not controllable"
    return False, state.last


async def _ask_ranked(target_id: str, located: _Located) -> tuple[bool, str]:
    state = _Attempt()
    peers = ranked_ids(
        target_id, located.found.props_by_id, located.found.names, located.receiver_name
    )
    for ident in peers:
        done = await _ask_one(state, ident, target_id, located)
        if done is not None:
            return done
    return _attempt_result(state, located.found.props_by_id.get(target_id, {}))


async def _join(target_id: str, receiver_id: str) -> tuple[bool, str]:
    located = await _locate(target_id, receiver_id)
    if located.error:
        return False, located.error
    if phone_owns_playback(target_id, located.found.props_by_id):
        return False, "another device is sending audio"
    return await _ask_ranked(target_id, located)


def join(target_id: str, receiver_id: str) -> tuple[bool, str]:
    if not target_id or not receiver_id:
        return False, "target_id and receiver_id are required"
    try:
        return asyncio.run(_join(target_id, receiver_id))
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


def _listen_address() -> tuple[str, int]:
    host, _, port = LISTEN.rpartition(":")
    if not host:
        host = "127.0.0.1"
    return host, int(port)


def _is_loopback(host: str) -> bool:
    if host == "localhost":
        return True
    try:
        return ip_address(host).is_loopback
    except ValueError:
        return False


def main() -> None:
    host, port = _listen_address()
    if not _is_loopback(host):
        raise SystemExit("Auto Join sidecar listens on loopback only")
    server = ThreadingHTTPServer((host, port), Handler)
    threading.Thread(target=_scan_loop, name="autojoin-scan", daemon=True).start()
    # Loopback IPC inside the container. S5332 flags this method for every HTTPServer.
    server.serve_forever()  # NOSONAR


if __name__ == "__main__":
    main()
