#!/usr/bin/env python3
"""Localhost Auto Join control plane.

Binds 127.0.0.1 only. WLEDPlay's operator API remains the only UI.
A standing Avahi browser fills the target snapshot and the session table.
Speakers that appear after the first pass are added. Rescan replaces the list.
This receiver is resolved again until its advertisement is stored. That
lookup reuses its resolver slot and does not replace the target list.
pyatv runs only when a join is sent. Credentials stay in a pyatv settings
file. PYATV_STORAGE may be that file or a directory; a directory is stored
as <dir>/pyatv.json.

GET  /v1/scan?name=WLEDPlay&probe=<target-id>
POST /v1/rescan
GET  /v1/wled
POST /v1/wled/rescan
POST /v1/join   {"target_id","receiver_id"}
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import threading
import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import ip_address, ip_network
from urllib.parse import parse_qs, urlparse

LISTEN = os.environ.get("AUTOJOIN_LISTEN", "127.0.0.1:8091")
STORAGE = os.environ.get("PYATV_STORAGE", "/host-config/pyatv")
RECEIVER_NAME = os.environ.get("WLEDPLAY_AIRPLAY_NAME", "WLEDPlay")
AIRPLAY_IFACE = os.environ.get("WLEDPLAY_AIRPLAY_INTERFACE", "").strip()
UNICAST_IFACE = os.environ.get("WLEDPLAY_UNICAST_INTERFACE", "").strip()
GROUP_UNSUPPORTED = "group modification not supported"

_lock = threading.Lock()
_records: dict = {}
_snapshot: list = []
_snapshot_at = ""
_self_id = ""
_discovery_active = False
_discovery_error = ""
_probe = {"id": "", "name": RECEIVER_NAME}
_index = {"devices": {}, "self_deviceid": "", "self_name": ""}
_browser = None
_wled_rows: dict = {}


def airplay_network():
    """The AirPlay NIC's IPv4 network, looked up once per process."""
    if getattr(airplay_network, "ready", False):
        return airplay_network.value
    airplay_network.value = iface_network(AIRPLAY_IFACE)
    airplay_network.ready = True
    return airplay_network.value


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


def keep_resolved_address(label: str, address: str, receiver_name: str, network) -> bool:
    """Keep an address from a resolver.

    Other speakers must sit on the AirPlay NIC. This host's own service can
    resolve to the other NIC, and that advertisement is still this receiver.
    """
    if not address:
        return False
    if _is_receiver_name(label, receiver_name):
        return True
    return address_on_network(address, network)


def _current_self_id() -> str:
    with _lock:
        return _self_id


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


def _id_forms(value: str) -> list[str]:
    """Each literal UID a leader might match. Colon and compact forms are not the same string."""
    raw = (value or "").strip()
    if not raw:
        return []
    forms = [raw]
    mac = mac_device_id(raw)
    if mac and mac not in forms:
        forms.append(mac)
    if mac and ":" in mac:
        compact = mac.replace(":", "")
        if compact not in forms:
            forms.append(compact)
    return forms


def receiver_output_ids(deviceid: str, fallback: str, extras: list[str] | None = None) -> list[str]:
    """Ids to give add_output_devices. The leader matches the UID string exactly.

    Apple speakers are listed by their psi. Some receivers are listed by pi,
    which may be the device id. Both are tried before the deviceid text forms.
    """
    found: list[str] = []
    for value in [*(extras or []), deviceid, fallback]:
        for form in _id_forms(value):
            if form not in found:
                found.append(form)
    return found


def self_record_props(index: dict) -> dict:
    devices = index.get("devices") or {}
    self_name = (index.get("self_name") or "").lower()
    self_deviceid = index.get("self_deviceid") or ""
    for rec in devices.values():
        props = rec.get("props") or {}
        name = (rec.get("name") or "").lower()
        if self_name and name == self_name:
            return props
        if self_deviceid and ids_match(props.get("deviceid") or "", self_deviceid):
            return props
    return {}


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


def leader_session_is_playing(props: dict, props_by_id: dict) -> bool:
    """An origin leader does not set the receiving bits while it is playing.

    Those bits are set on the speakers taking its stream. A session whose
    members are not receiving is paused.
    """
    if not is_group_leader_txt(props):
        return False
    for peer in props_by_id.values():
        if peer is props:
            continue
        if not (shares_session(peer, props) or shares_session(props, peer)):
            continue
        if receiving_audio(peer):
            return True
    return False


def playback_kind(props: dict, props_by_id: dict | None = None) -> str:
    """playing, paused, or idle from the TXT record. No playback connection."""
    peers = props_by_id or {}
    if receiving_audio(props) or leader_session_is_playing(props, peers):
        return "playing"
    if session_ids(props):
        return "paused"
    return "idle"


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


def snapshot_row(ident: str, rec: dict) -> dict:
    return {
        "id": ident,
        "name": rec.get("name") or ident,
        "model": rec.get("model") or "",
        "address": rec.get("address") or "",
    }


def include_snapshot(rows: list, ident: str, rec: dict, receiver_name: str) -> bool:
    """Add or refresh one speaker. This receiver stays out of the target list."""
    if not rec.get("present"):
        return False
    if _is_receiver_name(rec.get("name") or "", receiver_name):
        return False
    row = snapshot_row(ident, rec)
    for existing in rows:
        if existing.get("id") != ident:
            continue
        if existing == row:
            return False
        existing.clear()
        existing.update(row)
        return True
    rows.append(row)
    rows.sort(key=lambda item: (item.get("name") or "").casefold())
    return True


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


def _player_row(ident: str, props: dict, names: dict, target_id: str) -> dict:
    return {
        "name": names.get(ident) or ident,
        "flags": (props.get("flags") or props.get("sf") or ""),
        "igl": props.get("igl") or "",
        "gcgl": props.get("gcgl") or "",
        "target": ident == target_id,
        "leader": is_group_leader_txt(props),
    }


def session_players(
    probe_id: str, props_by_id: dict, names: dict
) -> list[dict]:
    """Every speaker in the selected session, including this player."""
    if not probe_id:
        return []
    leaders, others = session_peer_ids(probe_id, props_by_id, names, "", set(), True)
    idents = [probe_id]
    for ident in leaders + others:
        if ident not in idents:
            idents.append(ident)
    rows = [
        _player_row(ident, props_by_id.get(ident) or {}, names, probe_id)
        for ident in idents
        if names.get(ident) or ident
    ]
    rows.sort(key=lambda row: row["name"].casefold())
    return rows


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
    members.sort(key=str.casefold)
    return {
        "target_name": target_name,
        "flags": (mine.get("flags") or mine.get("sf") or ""),
        "igl": mine.get("igl") or "",
        "gcgl": mine.get("gcgl") or "",
        "gid": mine.get("gid") or "",
        "leader": leader,
        "members": members,
        "players": session_players(probe_id, props_by_id, names),
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


async def scan_hosts(storage, hosts: list[str]):
    """Directed lookup of known addresses. An empty list does not scan the LAN."""
    import pyatv

    cleaned = [host for host in hosts if host]
    if not cleaned:
        return []
    loop = asyncio.get_running_loop()
    try:
        return await pyatv.scan(loop, timeout=2, hosts=cleaned, storage=storage)
    except TypeError:
        return await pyatv.scan(timeout=2, hosts=cleaned, storage=storage)


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


async def _playback_result(conf, storage) -> tuple[str, str]:
    try:
        return await probe_playback(conf, storage), ""
    except Exception as exc:
        return "idle", str(exc)


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


def alias_target(found: ScanIndex, target_id: str, address: str) -> None:
    """Point the mDNS target id at the pyatv config for that speaker."""
    if not target_id or target_id in found.by_id:
        return
    match = ""
    for ident, props in found.props_by_id.items():
        if ids_match(ident, target_id) or ids_match(props.get("deviceid") or "", target_id):
            match = ident
            break
    if not match and address:
        for ident, meta in found.meta.items():
            if (meta.get("address") or "") == address:
                match = ident
                break
    if not match or match not in found.by_id:
        return
    found.by_id[target_id] = found.by_id[match]
    found.props_by_id[target_id] = found.props_by_id.get(match) or {}
    found.names[target_id] = found.names.get(match, "")


async def _scan_for_target(storage, target_id: str, receiver_name: str, network, index: dict):
    hosts = leader_hosts(target_id, index, receiver_name)
    if not hosts:
        empty = collect_confs([], receiver_name, network)
        return empty, _remembered_deviceid("", index)
    found = collect_confs(await scan_hosts(storage, hosts), receiver_name, network)
    alias_target(found, target_id, hosts[0])
    return found, _remembered_deviceid(found.self_deviceid, index)


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
    own = self_record_props(index)
    output_ids = receiver_output_ids(
        self_deviceid,
        receiver_id,
        [own.get("psi") or "", own.get("pi") or ""],
    )
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


def _copy_records(records: dict) -> dict:
    copied = {}
    for ident, rec in records.items():
        copied[ident] = {
            "name": rec.get("name") or "",
            "address": rec.get("address") or "",
            "model": rec.get("model") or "",
            "present": bool(rec.get("present")),
            "props": dict(rec.get("props") or {}),
        }
    return copied


def _store_live(records: dict) -> None:
    """Publish the live cache and the join index. The dropdown snapshot is separate."""
    global _records, _self_id
    copied = _copy_records(records)
    name = _receiver_name()
    self_id = ""
    self_deviceid = ""
    meta = {}
    for ident, rec in copied.items():
        if not rec["present"]:
            continue
        props = rec["props"]
        if not self_id and _is_receiver_name(rec["name"], name):
            self_id = ident
            self_deviceid = props.get("deviceid") or ""
        meta[ident] = {"address": rec["address"], "name": rec["name"], "props": props}
    remember_index(meta, self_deviceid, name)
    with _lock:
        _records = copied
        _self_id = self_id


def _replace_snapshot(records: dict) -> None:
    global _snapshot, _snapshot_at
    name = _receiver_name()
    rows = []
    for ident, rec in records.items():
        include_snapshot(rows, ident, rec, name)
    with _lock:
        _snapshot = rows
        _snapshot_at = time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime())


def _fold_snapshot(ident: str, rec: dict) -> None:
    """Keep a speaker that arrives after the first pass on the target list."""
    global _snapshot, _snapshot_at
    name = _receiver_name()
    with _lock:
        rows = [dict(row) for row in _snapshot]
        stamp = _snapshot_at
    if not include_snapshot(rows, ident, rec, name):
        return
    with _lock:
        _snapshot = rows
        if not stamp:
            _snapshot_at = time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime())


def scan_payload() -> dict:
    """GET /v1/scan reads the cache. It does not start a lookup."""
    with _lock:
        records = _copy_records(_records)
        snapshot = [dict(row) for row in _snapshot]
        self_id = _self_id
        active = _discovery_active
        error = _discovery_error
        snapshot_at = _snapshot_at
        probe = _probe.get("id") or ""
        receiver = _probe.get("name") or RECEIVER_NAME
    present = {ident: rec for ident, rec in records.items() if rec.get("present")}
    props_by_id = {ident: rec["props"] for ident, rec in present.items()}
    names = {ident: rec["name"] for ident, rec in present.items()}
    devices = []
    for row in snapshot:
        ident = row["id"]
        live = present.get(ident)
        props = live["props"] if live else {}
        kind = playback_kind(props, props_by_id) if live else "idle"
        devices.append(
            {
                "id": ident,
                "name": row.get("name") or ident,
                "model": row.get("model") or "",
                "address": (live or row).get("address") or "",
                "available": live is not None,
                "playing": kind == "playing",
                "paused": kind == "paused",
                "airplay2": True,
                "phone_owned": phone_owns_playback(ident, props_by_id) if live else False,
            }
        )
    return {
        "self_id": self_id,
        "discovery_active": active,
        "error": error,
        "devices": devices,
        "session": session_view(probe, props_by_id, names, receiver),
        "snapshot_at": snapshot_at,
    }


def decode_txt(txt) -> dict:
    props = {}
    for entry in list(txt or []):
        raw = bytes(int(part) & 0xFF for part in entry)
        text = raw.decode("utf-8", "replace")
        if "=" not in text:
            continue
        key, value = text.split("=", 1)
        props[key.strip().lower()] = value.strip()
    return props


def service_label(name: str, stype: str) -> str:
    if stype.startswith("_raop") and "@" in name:
        return name.split("@", 1)[1]
    return name


def raop_instance_id(service_name: str) -> str:
    """RAOP instance names are <12 hex>@Speaker. That prefix is the device id."""
    if "@" not in service_name:
        return ""
    mac = mac_device_id(service_name.split("@", 1)[0])
    if mac and ":" in mac:
        return mac
    return ""


def stable_id(props: dict, service_name: str) -> str:
    deviceid = props.get("deviceid") or ""
    mac = mac_device_id(deviceid)
    if mac and ":" in mac:
        return mac
    from_name = raop_instance_id(service_name)
    if from_name:
        return from_name
    if deviceid:
        return deviceid.strip()
    return service_name


class AvahiBrowser:
    """One browser for the life of the sidecar. Rescan starts a new pass."""

    def __init__(self) -> None:
        self.loop = None
        self._glib = None
        self.bus = None
        self.server = None
        self.browsers = []
        self.matches = []
        self.resolvers = {}
        self.records = {}
        self.services = {}
        self.pending = set()
        self.unresolved = set()
        self.generation = 0
        self.snapshot_open = False
        self.pass_error = ""
        self.ready = threading.Event()
        self.wled_browsers = []
        self.wled_matches = []
        self.wled_resolvers = {}
        self.wled_resolver_signals = {}
        self.wled_generation = 0
        self.wled_network = None
        self.receiver_watch = False
        self.receiver_signals = []
        self.resolver_signals = {}
        self.resolve_attempt = {}

    def start(self) -> None:
        threading.Thread(target=self._run, name="avahi-browse", daemon=True).start()
        self.ready.wait(timeout=5)

    def request_rescan(self) -> bool:
        glib = self._glib
        if glib is None or self.loop is None or not self.loop.is_running():
            return False
        glib.idle_add(self._restart)
        return True

    def request_wled_rescan(self) -> bool:
        glib = self._glib
        if glib is None or self.loop is None or not self.loop.is_running():
            return False
        glib.idle_add(self._restart_wled)
        return True

    def _run(self) -> None:
        try:
            from dbus.mainloop.glib import DBusGMainLoop
            from gi.repository import GLib
            import dbus

            DBusGMainLoop(set_as_default=True)
            self._glib = GLib
            self._dbus = dbus
            self.bus = dbus.SystemBus()
            self.server = dbus.Interface(
                self.bus.get_object("org.freedesktop.Avahi", "/"),
                "org.freedesktop.Avahi.Server",
            )
            self._open_browsers()
            self._open_wled()
            self.loop = GLib.MainLoop()
            self.ready.set()
            self.loop.run()
        except Exception as exc:
            self._set_status(False, str(exc))
            self.ready.set()

    def _set_status(self, active: bool, error: str) -> None:
        global _discovery_active, _discovery_error
        with _lock:
            _discovery_active = active
            _discovery_error = error

    def _restart(self) -> bool:
        self.generation += 1
        self.receiver_watch = False
        self._drop_clients()
        self.records.clear()
        self.services.clear()
        self.unresolved.clear()
        self._publish_live()
        self._open_browsers()
        return False

    def _interface_index(self, name: str) -> int:
        """Avahi's index for one NIC. -1 browses every interface Avahi allows."""
        if not name or self.server is None:
            return -1
        try:
            return int(self.server.GetNetworkInterfaceIndexByName(name))
        except Exception:
            return -1

    def _open_browsers(self) -> None:
        self.pass_error = ""
        self.pending = set()
        self.unresolved = set()
        self.snapshot_open = True
        opened = 0
        # Speakers are on the AirPlay NIC. A browse of every allowed NIC also
        # sees that name on the unicast NIC, and one resolver slot would keep
        # the TXT record from the NIC that answered first.
        iface = self._dbus.Int32(self._interface_index(AIRPLAY_IFACE))
        for stype in ("_airplay._tcp", "_raop._tcp"):
            try:
                path = self.server.ServiceBrowserNew(
                    iface,
                    self._dbus.Int32(0),
                    stype,
                    "local",
                    self._dbus.UInt32(0),
                )
                browser = self._dbus.Interface(
                    self.bus.get_object("org.freedesktop.Avahi", path),
                    "org.freedesktop.Avahi.ServiceBrowser",
                )
                self._listen(
                    browser,
                    "ItemNew",
                    lambda *args, kind=stype, gen=self.generation: self._on_new(gen, kind, *args),
                )
                self._listen(
                    browser,
                    "ItemRemove",
                    lambda *args, kind=stype, gen=self.generation: self._on_remove(
                        gen, kind, *args
                    ),
                )
                self._listen(
                    browser,
                    "AllForNow",
                    lambda kind=stype, gen=self.generation: self._finish_browser(gen, kind),
                )
                self._listen(
                    browser,
                    "Failure",
                    lambda message, kind=stype, gen=self.generation: self._fail_browser(
                        gen, kind, message
                    ),
                )
                self.browsers.append((str(path), browser))
                self.pending.add(stype)
                opened += 1
            except Exception as exc:
                self.pass_error = str(exc)
        self._set_status(opened > 0, self.pass_error)
        self._maybe_publish()
        self._arm_receiver_watch()

    def _listen(self, obj, signal: str, handler, bucket=None) -> None:
        held = self.matches if bucket is None else bucket
        held.append(obj.connect_to_signal(signal, handler))

    def _drop_signals(self, held: list) -> None:
        for match in held:
            try:
                match.remove()
            except Exception:
                pass
        held.clear()

    def _drop_clients(self) -> None:
        self._drop_signals(self.matches)
        self._drop_signals(self.receiver_signals)
        for key in list(self.resolver_signals):
            self._drop_resolver_signals(key)
        for path, _browser_obj in self.browsers:
            self._free(path, "ServiceBrowser")
        for path in list(self.resolvers.values()):
            if path:
                self._free(path, "ServiceResolver")
        self.browsers.clear()
        self.resolvers.clear()
        self.resolve_attempt.clear()

    def _drop_resolver_signals(self, key) -> None:
        self._drop_signals(self.resolver_signals.pop(key, []))

    def _release_resolver(self, key, keep_slot: bool = False) -> None:
        self._drop_resolver_signals(key)
        path = self.resolvers.pop(key, "")
        if path:
            self._free(path, "ServiceResolver")
        if keep_slot:
            self.resolvers[key] = ""

    def _free(self, path: str, kind: str) -> None:
        try:
            obj = self.bus.get_object("org.freedesktop.Avahi", path)
            self._dbus.Interface(obj, "org.freedesktop.Avahi." + kind).Free()
        except Exception:
            return

    def _on_new(self, gen, _kind, interface, protocol, name, stype, domain, _flags) -> None:
        if gen != self.generation:
            return
        self._resolve(
            int(interface), int(protocol), str(name), str(stype), str(domain), refresh=True
        )

    def _on_remove(self, gen, _kind, _interface, _protocol, name, stype, _domain, _flags) -> None:
        if gen != self.generation:
            return
        key = (int(_interface), str(name), str(stype))
        ident = self.services.pop(key, "")
        if not ident:
            return
        if any(other == ident for other in self.services.values()):
            self._publish_live()
            return
        rec = self.records.get(ident)
        if rec is not None:
            rec["present"] = False
        self._publish_live()

    def _resolve(
        self,
        interface: int,
        protocol: int,
        name: str,
        stype: str,
        domain: str,
        refresh: bool = False,
        gate: bool = True,
    ) -> None:
        key = (interface, name, stype)
        occupied = key in self.resolvers or key in self.unresolved
        if occupied and not refresh:
            return
        self._drop_resolver_signals(key)
        old = self.resolvers.pop(key, "")
        self.unresolved.discard(key)
        if old:
            self._free(old, "ServiceResolver")
        attempt = self.resolve_attempt.get(key, 0) + 1
        self.resolve_attempt[key] = attempt
        try:
            path = self.server.ServiceResolverNew(
                self._dbus.Int32(interface),
                self._dbus.Int32(protocol),
                name,
                stype,
                domain,
                self._dbus.Int32(0),
                self._dbus.UInt32(0),
            )
        except Exception as exc:
            self.pass_error = str(exc)
            self._set_status(True, self.pass_error)
            self._maybe_publish()
            return
        path = str(path)
        if gate:
            self.unresolved.add(key)
        try:
            resolver = self._dbus.Interface(
                self.bus.get_object("org.freedesktop.Avahi", path),
                "org.freedesktop.Avahi.ServiceResolver",
            )
            bucket = []
            self.resolver_signals[key] = bucket
            self._listen(
                resolver,
                "Found",
                lambda *args, key=key, gen=self.generation, attempt=attempt: self._on_found(
                    gen, key, attempt, *args
                ),
                bucket,
            )
            self._listen(
                resolver,
                "Failure",
                lambda message, key=key, gen=self.generation, attempt=attempt: self._on_resolver_failure(
                    gen, key, attempt, message
                ),
                bucket,
            )
            self.resolvers[key] = path
        except Exception as exc:
            self._release_resolver(key)
            self._free(path, "ServiceResolver")
            self.pass_error = str(exc)
            self._set_status(True, self.pass_error)
            self._note_resolved(key)
        if not gate:
            self._maybe_publish()

    def _on_found(
        self, gen, key, attempt, _interface, _protocol, name, stype, _domain, _host, _aprotocol,
        address, _port, txt, _flags,
    ) -> None:
        if gen != self.generation or attempt != self.resolve_attempt.get(key):
            return
        try:
            self._apply_found(key, name, stype, address, txt)
        finally:
            self._release_resolver(key, keep_slot=True)
            self._note_resolved(key)

    def _apply_found(self, key, name, stype, address, txt) -> None:
        props = decode_txt(txt)
        text = str(address or "").strip()
        service_name = str(name)
        service_type = str(stype)
        label = service_label(service_name, service_type)
        if not keep_resolved_address(label, text, _receiver_name(), airplay_network()):
            return
        ident = stable_id(props, service_name)
        if not ident:
            return
        is_raop = service_type.startswith("_raop")
        rec = self.records.get(ident)
        if rec is None:
            rec = {
                "name": label,
                "address": text,
                "model": props.get("model") or "",
                "present": True,
                "props": props,
                "named_by_airplay": not is_raop,
            }
            self.records[ident] = rec
        else:
            rec["present"] = True
            rec["address"] = text or rec.get("address") or ""
            if is_raop and rec.get("named_by_airplay"):
                merged = dict(props)
                merged.update(rec.get("props") or {})
                rec["props"] = merged
            else:
                rec["props"] = props
            rec["model"] = props.get("model") or rec.get("model") or ""
            if not is_raop or not rec.get("named_by_airplay"):
                rec["name"] = label
                rec["named_by_airplay"] = not is_raop
        self.services[key] = ident
        self._publish_live()
        if not self.snapshot_open:
            _fold_snapshot(ident, rec)

    def _on_resolver_failure(self, gen, key, attempt, _message) -> None:
        if gen != self.generation or attempt != self.resolve_attempt.get(key):
            return
        self._release_resolver(key, keep_slot=True)
        self._note_resolved(key)

    def _note_resolved(self, key) -> None:
        if key not in self.unresolved:
            return
        self.unresolved.discard(key)
        self._maybe_publish()

    def _fail_browser(self, gen, stype: str, message) -> None:
        if gen != self.generation:
            return
        self.pass_error = str(message)
        self._set_status(True, self.pass_error)
        self._finish_browser(gen, stype)

    def _finish_browser(self, gen, stype: str) -> None:
        if gen != self.generation:
            return
        self.pending.discard(stype)
        self._maybe_publish()

    def _maybe_publish(self) -> None:
        if not self.snapshot_open or self.pending or self.unresolved:
            return
        self.snapshot_open = False
        if self.browsers:
            self._publish_snapshot()

    def _restart_wled(self) -> bool:
        self.wled_generation += 1
        self._drop_wled()
        with _lock:
            _wled_rows.clear()
        self._open_wled()
        return False

    def _open_wled(self) -> None:
        self.wled_network = iface_network(UNICAST_IFACE)
        stype = "_wled._tcp"
        iface = self._dbus.Int32(self._interface_index(UNICAST_IFACE))
        try:
            path = self.server.ServiceBrowserNew(
                iface,
                self._dbus.Int32(0),
                stype,
                "local",
                self._dbus.UInt32(0),
            )
            browser = self._dbus.Interface(
                self.bus.get_object("org.freedesktop.Avahi", path),
                "org.freedesktop.Avahi.ServiceBrowser",
            )
            gen = self.wled_generation
            self._listen_wled(
                browser,
                "ItemNew",
                lambda *args, generation=gen: self._on_wled_new(generation, *args),
            )
            self._listen_wled(
                browser,
                "ItemRemove",
                lambda *args, generation=gen: self._on_wled_remove(generation, *args),
            )
            self.wled_browsers.append(str(path))
        except Exception:
            return

    def _listen_wled(self, obj, signal: str, handler) -> None:
        self.wled_matches.append(obj.connect_to_signal(signal, handler))

    def _drop_wled(self) -> None:
        for match in self.wled_matches:
            try:
                match.remove()
            except Exception:
                pass
        self.wled_matches.clear()
        for path in self.wled_browsers:
            self._free(path, "ServiceBrowser")
        for path in list(self.wled_resolvers.values()):
            if path:
                self._free(path, "ServiceResolver")
        self.wled_browsers.clear()
        self.wled_resolvers.clear()
        self.wled_resolver_signals.clear()

    def _listen_wled(self, obj, signal: str, handler, bucket=None) -> None:
        match = obj.connect_to_signal(signal, handler)
        self.wled_matches.append(match)
        if bucket is not None:
            bucket.append(match)

    def _drop_wled_resolver_signals(self, key) -> None:
        matches = self.wled_resolver_signals.pop(key, [])
        if not matches:
            return
        match_ids = {id(match) for match in matches}
        self._drop_signals(matches)
        self.wled_matches[:] = [match for match in self.wled_matches if id(match) not in match_ids]

    def _release_wled_resolver(self, key, keep_slot: bool = False) -> None:
        self._drop_wled_resolver_signals(key)
        path = self.wled_resolvers.pop(key, "")
        if path:
            self._free(path, "ServiceResolver")
        if keep_slot:
            self.wled_resolvers[key] = ""

    def _on_wled_new(self, gen, interface, protocol, name, stype, domain, _flags) -> None:
        if gen != self.wled_generation:
            return
        key = (int(interface), str(name), str(stype))
        if key in self.wled_resolvers:
            return
        try:
            path = self.server.ServiceResolverNew(
                self._dbus.Int32(int(interface)),
                self._dbus.Int32(int(protocol)),
                str(name),
                str(stype),
                str(domain),
                self._dbus.Int32(0),
                self._dbus.UInt32(0),
            )
        except Exception:
            return
        path = str(path)
        self.wled_resolvers[key] = path
        bucket = []
        self.wled_resolver_signals[key] = bucket
        try:
            resolver = self._dbus.Interface(
                self.bus.get_object("org.freedesktop.Avahi", path),
                "org.freedesktop.Avahi.ServiceResolver",
            )
            self._listen_wled(
                resolver,
                "Found",
                lambda *args, generation=gen, ident=key: self._on_wled_found(generation, ident, *args),
                bucket,
            )
        except Exception:
            self._release_wled_resolver(key)

    def _on_wled_remove(self, gen, _interface, _protocol, name, stype, _domain, _flags) -> None:
        if gen != self.wled_generation:
            return
        key = (int(_interface), str(name), str(stype))
        self._release_wled_resolver(key)
        with _lock:
            for row in _wled_rows.values():
                if row.get("service") == key:
                    row["online"] = False

    def _on_wled_found(self, gen, key, _interface, _protocol, name, _stype, _domain, _host, _aprotocol, address, _port, _txt, _flags) -> None:
        if gen != self.wled_generation:
            return
        try:
            text = _ipv4_text(address)
            if not text or not address_on_network(text, self.wled_network):
                return
            with _lock:
                row = _wled_rows.get(text)
                if row is None:
                    _wled_rows[text] = {
                        "address": text,
                        "name": str(name),
                        "online": True,
                        "service": key,
                    }
                else:
                    row["name"] = str(name) or row.get("name") or ""
                    row["online"] = True
                    row["service"] = key
        finally:
            self._release_wled_resolver(key, keep_slot=True)

    def _publish_live(self) -> None:
        _store_live(self.records)
        if not _current_self_id():
            self._arm_receiver_watch()

    def _arm_receiver_watch(self) -> None:
        """Ask again for this receiver until Shairport's advertisement is stored."""
        glib = self._glib
        if self.receiver_watch or glib is None:
            return
        self.receiver_watch = True
        glib.idle_add(self._resolve_named_receiver)
        glib.timeout_add_seconds(2, self._poll_receiver, self.generation)

    def _poll_receiver(self, gen: int) -> bool:
        if gen != self.generation:
            return False
        if _current_self_id():
            self.receiver_watch = False
            return False
        self._resolve_named_receiver()
        return True

    def _resolve_named_receiver(self) -> bool:
        name = _receiver_name()
        if not name or self.server is None or _current_self_id():
            return False
        iface = self._interface_index(AIRPLAY_IFACE)
        key = (iface, name, "_airplay._tcp")
        # A lookup started before Shairport is advertising stays in this slot.
        # Replace it, including one that has not answered, so the real
        # advertisement can be stored. Keep gating the target list only when
        # a browse resolve for this name is already part of that wait.
        gate = key in self.unresolved
        self._resolve(iface, 0, name, "_airplay._tcp", "local", refresh=True, gate=gate)
        return False

    def _publish_snapshot(self) -> None:
        self._publish_live()
        _replace_snapshot(_copy_records(self.records))
        self._set_status(True, self.pass_error)


def _ipv4_text(addr) -> str:
    text = str(addr or "").strip()
    try:
        ip = ip_address(text)
    except ValueError:
        return ""
    if ip.version != 4:
        return ""
    return text


def wled_payload() -> dict:
    with _lock:
        rows = [
            {
                "address": row.get("address") or "",
                "name": row.get("name") or "",
                "online": bool(row.get("online")),
            }
            for row in _wled_rows.values()
        ]
    return {"ok": True, "controllers": rows}


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
        if parsed.path == "/v1/wled":
            self._send(200, wled_payload())
            return
        if parsed.path != "/v1/scan":
            self._send(404, {"ok": False, "error": "not found"})
            return
        qs = parse_qs(parsed.query)
        name = (qs.get("name") or [RECEIVER_NAME])[0]
        probe = (qs.get("probe") or [""])[0]
        with _lock:
            _probe["id"] = probe
            _probe["name"] = name
        self._send(200, scan_payload())

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path == "/v1/wled/rescan":
            ok = bool(_browser and _browser.request_wled_rescan())
            self._send(202 if ok else 503, {"ok": ok})
            return
        if parsed.path == "/v1/rescan":
            ok = bool(_browser and _browser.request_rescan())
            self._send(202 if ok else 503, {"ok": ok})
            return
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
    global _browser
    _browser = AvahiBrowser()
    _browser.start()
    server = ThreadingHTTPServer((host, port), Handler)
    # Loopback IPC inside the container. S5332 flags this method for every HTTPServer.
    server.serve_forever()  # NOSONAR


if __name__ == "__main__":
    main()
