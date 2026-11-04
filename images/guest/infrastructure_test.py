"""Fixed Linux reference cases; all test state stays private and disposable."""
import hashlib
import ipaddress
import json
import struct
import uuid
from typing import TYPE_CHECKING

if __package__ or TYPE_CHECKING:
    from .boot_test import boot_case, parse_result
    from .build import encoded, image_info
    from .lock import file_hash, load_lock, require
else:
    from boot_test import boot_case, parse_result
    from build import encoded, image_info
    from lock import file_hash, load_lock, require


def network_evidence(path):
    packets = external_tcp = 0
    queries = set()
    with path.open('rb') as stream:
        header = stream.read(24)
        require(len(header) == 24 and header[:4] in (b'\xd4\xc3\xb2\xa1', b'\xa1\xb2\xc3\xd4'), 'invalid pcap header')
        endian = '<' if header[:4] == b'\xd4\xc3\xb2\xa1' else '>'
        require(struct.unpack(endian + 'I', header[20:24])[0] == 1, 'expected Ethernet capture')
        while record := stream.read(16):
            require(len(record) == 16, 'truncated capture')
            size = struct.unpack(endian + 'IIII', record)[2]
            require(size <= 65535, 'oversized packet')
            frame = stream.read(size)
            require(len(frame) == size, 'truncated packet')
            packets += 1
            if len(frame) < 34 or frame[12:14] != b'\x08\x00':
                continue
            ihl = (frame[14] & 15) * 4
            destination = ipaddress.ip_address(frame[30:34])
            if frame[23] == 6 and destination != ipaddress.ip_address('10.0.2.15') and not destination.is_multicast:
                external_tcp += 1
            udp = 14 + ihl
            if frame[23] == 17 and len(frame) > udp + 20 and frame[udp + 2:udp + 4] == b'\x00\x35':
                labels, offset = [], udp + 20
                while offset < len(frame) and 0 < frame[offset] < 64:
                    length = frame[offset]
                    offset += 1
                    labels.append(frame[offset:offset + length].decode('ascii', errors='replace'))
                    offset += length
                if labels:
                    queries.add('.'.join(labels))
    return {'captureSha256': file_hash(path), 'packets': packets, 'externalIpv4TcpPackets': external_tcp, 'dnsQueries': sorted(queries), 'qemuRestrictedUserNet': True}


def infrastructure(image, lock_path, out, case_name, fixture=None, from_receipt=None):
    require(case_name != 'interrupted' or from_receipt is None, 'interrupted case requires exclusively new state')
    lock = load_lock(lock_path)
    build = json.loads(image.with_name('build.json').read_bytes())
    require(build['sha256'] == file_hash(image) and build['inputSha256'] == hashlib.sha256(encoded(lock)).hexdigest(), 'factory input mismatch')
    require(image_info(image)['virtual-size'] == 16 * 1024**3 and lock['architecture'] == 'amd64', 'unsupported reference disk')
    require(not out.exists(), 'output already exists')
    prior = json.loads(from_receipt.read_bytes()) if from_receipt is not None else None
    if prior is not None:
        result = prior.get('new', {}).get('result', {})
        fingerprints = result.get('fingerprints', {})
        required = {'nodeUID', 'namespaceUID', 'snapshotUID', 'snapshotDataSha256', 'nodeIds', '/etc/kubernetes/pki/ca.crt', '/etc/kubernetes/pki/sa.pub', '/etc/kubernetes/pki/etcd/ca.crt'}
        require(prior.get('schemaVersion') == 'bridge-guest-infrastructure/v1' and prior.get('case') in ('infrastructure', 'restart') and result.get('phase') == 'infrastructure-ready' and isinstance(fingerprints, dict) and required <= fingerprints.keys(), 'completed infrastructure identity evidence required')
        require(prior['build']['inputSha256'] == build['inputSha256'] and prior['build']['contract'] == build['contract'], 'prior software/disk contract mismatch')
    out.mkdir()
    installation = prior['new']['bootDescriptor']['installationId'] if prior else str(uuid.uuid4())
    existing = from_receipt.parent / 'new/state.qcow2' if from_receipt is not None else None
    first = boot_case(image, out / 'new', installation, 'existing' if prior else 'new', existing, case_name=case_name, fixture=fixture)
    first['network'] = network_evidence(out / 'new/network.pcap')
    require(first['network']['externalIpv4TcpPackets'] == 0, 'external HTTP traffic attempted')
    require(not first['network']['dnsQueries'], 'external DNS traffic attempted')
    serial = first.pop('serial')
    base = parse_result(serial, installation)
    lines = [line[len('BRIDGE_INFRA '):] for line in serial.splitlines() if line.startswith('BRIDGE_INFRA ')]
    require(len(lines) == 1, 'infrastructure result missing')
    result = json.loads(lines[0])
    expected_phase = 'cluster-ready' if case_name == 'interrupted' else 'infrastructure-ready'
    require(result['phase'] == expected_phase and len(first['runtimeFrames']) == 6, 'infrastructure handoff incomplete')
    if prior is not None:
        require(result['fingerprints'] == prior['new']['result']['fingerprints'], 'persistent identities changed')
    record = {'schemaVersion': 'bridge-guest-infrastructure/v1', 'case': case_name, 'backend': 'qemu-kvm', 'publicationReady': False,
              'build': build, 'new': {**first, 'baseResult': base, 'result': result}}
    if case_name in ('infrastructure', 'restart', 'interrupted'):
        second_id = installation if case_name != 'infrastructure' else str(uuid.uuid4())
        second = boot_case(image, out / 'second', second_id, 'new' if case_name == 'infrastructure' else 'existing',
                           None if case_name == 'infrastructure' else out / 'new/state.qcow2',
                           case_name='interrupted' if case_name == 'interrupted' else 'infrastructure',
                           prior=out / 'new/state.qcow2' if case_name == 'infrastructure' else None)
        second['network'] = network_evidence(out / 'second/network.pcap')
        require(second['network']['externalIpv4TcpPackets'] == 0, 'external HTTP traffic attempted')
        require(not second['network']['dnsQueries'], 'external DNS traffic attempted')
        serial = second.pop('serial')
        second['baseResult'] = parse_result(serial, second_id)
        lines = [line[len('BRIDGE_INFRA '):] for line in serial.splitlines() if line.startswith('BRIDGE_INFRA ')]
        require(len(lines) == 1 and len(second['runtimeFrames']) == 6, 'second boot proof missing')
        second['result'] = json.loads(lines[0])
        if case_name == 'infrastructure':
            require(second['result'].get('priorCredentialsRejectedOnReachableTarget') is True, 'cross-install auth not proven')
            require(second['result']['fingerprints']['/etc/kubernetes/pki/ca.crt'] != result['fingerprints']['/etc/kubernetes/pki/ca.crt'], 'cluster CA was cloned')
        elif case_name == 'restart':
            require(second['result']['fingerprints'] == result['fingerprints'], 'restart changed persistent identity')
        else:
            require(second['result'].get('partialInstallRefused') is True and second['result'].get('identityPreserved') is True, 'partial install refusal not proven')
        record['second'] = second
    (out / 'boot.json').write_bytes(encoded(record) + b'\n')
    return record
