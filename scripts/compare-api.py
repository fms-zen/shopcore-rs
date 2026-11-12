#!/usr/bin/env python3
"""Compare isolated, identically seeded ShopCore APIs. Mutates both databases.

Only generated tokens and validated creation timestamps are nondeterministic.
IDs, row order, money, missing keys and errors remain meaningful. Named safety
policies below validate specific documented differences, never arbitrary mismatches.
Coupon dates cannot be configured through HTTP, so only seeded codes are exercised.
"""
import argparse
import copy
import csv
import datetime as dt
import email.utils
import gzip
import hashlib
import hmac
import io
import json
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass


OPERATIONS = {
    'GET /health', 'POST /api/register', 'POST /api/login', 'POST /api/logout',
    'GET /api/users/me', 'PATCH /api/users/me', 'GET /api/products',
    'POST /api/products', 'GET /api/products/{id}', 'PUT /api/products/{id}',
    'POST /api/orders', 'GET /api/orders', 'GET /api/orders/{id}',
    'GET /api/orders/{id}/invoice', 'POST /api/orders/{id}/cancel',
    'POST /api/orders/{id}/refund', 'POST /api/orders/{id}/pay',
    'GET /api/wallet', 'POST /api/wallet/topup', 'POST /api/wallet/transfer',
    'POST /api/webhooks/bank', 'GET /api/admin/users',
    'POST /api/admin/users/{id}/role', 'POST /api/admin/orders/{id}/status',
    'GET /api/reports/sales', 'POST /api/reports/export', 'GET /api/reports/download',
}


@dataclass
class Observation:
    label: str
    operation: str
    status: int
    body: object
    policy: str = ''


def timestamp(value):
    try:
        dt.datetime.fromisoformat(value)
    except ValueError:
        email.utils.parsedate_to_datetime(value)
    return '<generated timestamp>'


def normalize(value, key=''):
    if key == 'created_at' and isinstance(value, str):
        return timestamp(value)
    if key == 'token' and isinstance(value, str) and value:
        return '<generated token>'
    if isinstance(value, dict):
        return {k: normalize(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    return value


def equal(left, right):
    # JSON booleans must not compare equal to 0 or 1.
    if isinstance(left, bool) != isinstance(right, bool):
        return False
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(equal(left[k], right[k]) for k in left)
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(equal(a, b) for a, b in zip(left, right))
    return left == right


def comparable(observation):
    return {'status': observation.status, 'body': normalize(observation.body)}


def compare(legacy, rust):
    if (legacy.label, legacy.operation, legacy.policy) != (rust.label, rust.operation, rust.policy):
        return 'mismatch', 'observation identity differs'
    left, right = comparable(legacy), comparable(rust)
    policy = legacy.policy
    if policy == 'password_hash':
        if not isinstance(left['body'], list) or not isinstance(right['body'], list):
            return 'mismatch', 'admin user lists required'
        if any('password_hash' in row for row in right['body']):
            return 'mismatch', 'Rust exposed password_hash'
        for row in left['body']:
            if not isinstance(row.pop('password_hash', None), str):
                return 'mismatch', 'legacy password hash missing'
    if equal(left, right):
        return ('safety_difference' if policy == 'password_hash' else 'match'), policy
    # Scenario code supplies exact expected bodies for intentionally divergent cases.
    if policy and isinstance(legacy.body, dict) and '_expected' in legacy.body:
        expected_left, expected_right = legacy.body['_expected'], rust.body['_expected']
        if equal(left['body']['actual'], normalize(expected_left)) and equal(right['body']['actual'], normalize(expected_right)):
            return 'safety_difference', policy
    return 'mismatch', 'status or response differs'


class Client:
    def __init__(self, url):
        self.url = url.rstrip('/')
        self.observations = []

    def call(self, label, operation, path=None, body=None, token=None, policy='', headers=None):
        method, template = operation.split(' ', 1)
        request_headers = dict(headers or {})
        if token:
            request_headers['Authorization'] = 'Bearer ' + token
        raw = body if isinstance(body, bytes) else json.dumps(body).encode() if body is not None else None
        if raw is not None:
            request_headers['Content-Type'] = 'application/json'
        request = urllib.request.Request(self.url + (path or template), data=raw, headers=request_headers, method=method)
        try:
            response = urllib.request.urlopen(request, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            payload = response.read()
            if response.headers.get_content_type() == 'application/gzip':
                rows = list(csv.reader(io.StringIO(gzip.decompress(payload).decode())))
                if not rows or rows[0] != ['order_no', 'date', 'status', 'user', 'product', 'qty', 'price']:
                    raise ValueError(f'{label}: invalid CSV header {rows[:1]}')
                for row in rows[1:]:
                    if len(row) != 7:
                        raise ValueError(f'{label}: malformed CSV row {row}')
                    row[1] = timestamp(row[1])
                value = {'content_type': 'application/gzip', 'csv': rows}
            else:
                value = json.loads(payload)
            observation = Observation(label, operation, response.status, value, policy)
            self.observations.append(observation)
            return observation

    def known(self, observation, expected_legacy, expected_rust):
        observation.body = {'actual': {'status': observation.status, 'body': observation.body},
                            '_expected': expected_legacy if self.side == 'legacy' else expected_rust}


def scenario(client, suffix, secret, side):
    client.side = side
    def call(label, operation, path=None, body=None, token=None, policy=''):
        return client.call(label, operation, path, body, token, policy)
    def ok(observation, status=200):
        if observation.status != status:
            raise ValueError(f'{observation.label}: expected {status}, got {observation.status}: {observation.body}')
        return observation.body
    def order(label, items, **extra):
        return ok(call(label, 'POST /api/orders', body={'items': items, **extra}, token=alice), 201)
    call('health', 'GET /health')
    call('register omitted body', 'POST /api/register')
    call('login omitted body', 'POST /api/login')
    users = []
    for name in ['alice', 'bob', 'profile']:
        username = f'diff_{name}_{suffix}'
        users.append(ok(call(f'register {name}', 'POST /api/register', body={
            'username': username, 'email': username + '@example.test', 'password': 'comparison-password'}), 201))
    tokens = [ok(call('login ' + user['username'].split('_')[1], 'POST /api/login', body={
        'username': user['username'], 'password': 'comparison-password'}))['token'] for user in users]
    alice, bob, profile = tokens
    admin = ok(call('admin login', 'POST /api/login', body={'username': 'admin', 'password': 'admin123'}))['token']
    call('register explicit empty name', 'POST /api/register', body={'username': 'diff_empty_' + suffix, 'email': 'empty_' + suffix + '@example.test', 'password': 'comparison-password', 'name': ''})
    empty_token = ok(call('login empty-name user', 'POST /api/login', body={'username': 'diff_empty_' + suffix, 'password': 'comparison-password'}))['token']
    call('profile explicit empty name', 'GET /api/users/me', token=empty_token)
    call('profile initial', 'GET /api/users/me', token=alice)
    call('profile omitted body', 'PATCH /api/users/me', token=alice)
    call('profile name only', 'PATCH /api/users/me', body={'name': 'Comparison Alice'}, token=alice)
    call('profile email only', 'PATCH /api/users/me', body={'email': f'updated_{suffix}@example.test'}, token=alice)
    call('public catalog', 'GET /api/products')
    products = []
    for index, price in enumerate(['2.675', '1.125', '4.25']):
        products.append(ok(call(f'create decimal product {index}', 'POST /api/products', body={
            'sku': f'diff-{suffix}-{index}', 'name': f'Comparison {suffix} {index}', 'price': price, 'stock': 100}, token=admin), 201))
    pid = products[0]['id']
    call('product detail', 'GET /api/products/{id}', f'/api/products/{pid}')
    call('partial product update', 'PUT /api/products/{id}', f'/api/products/{pid}', {'stock': 90}, admin)
    call('product omitted update body', 'PUT /api/products/{id}', f'/api/products/{pid}', token=admin)
    query = urllib.parse.urlencode({'q': f'Comparison {suffix}', 'sort': 'price', 'page': 2, 'limit': 1})
    call('filtered catalog page two', 'GET /api/products', '/api/products?' + query)
    call('zero product limit', 'GET /api/products', '/api/products?limit=0')
    call('wallet initial', 'GET /api/wallet', token=alice)
    call('topup numeric string', 'POST /api/wallet/topup', body={'amount': '500.125'}, token=alice)
    call('transfer numeric string', 'POST /api/wallet/transfer', body={'to_user_id': str(users[1]['id']), 'amount': '10.25'}, token=alice)
    call('recipient wallet', 'GET /api/wallet', token=bob)
    call('order omitted body', 'POST /api/orders', token=alice)
    rounded = order('default quantity and fractional rounding', [{'product_id': pid}])
    mixed = order('mixed row quantities', [{'product_id': pid, 'quantity': 2}, {'product_id': products[1]['id'], 'quantity': '3'}])
    call('order detail', 'GET /api/orders/{id}', f"/api/orders/{mixed['id']}", token=alice)
    call('invoice JSON', 'GET /api/orders/{id}/invoice', f"/api/orders/{mixed['id']}/invoice", token=alice)
    call('orders page two', 'GET /api/orders', '/api/orders?page=2&limit=1', token=alice)
    call('other user order list', 'GET /api/orders', token=bob)
    call('cross user order detail retained policy', 'GET /api/orders/{id}', f"/api/orders/{mixed['id']}", token=bob)
    call('cross user invoice retained policy', 'GET /api/orders/{id}/invoice', f"/api/orders/{mixed['id']}/invoice", token=bob)
    call('cross user payment denied', 'POST /api/orders/{id}/pay', f"/api/orders/{mixed['id']}/pay", token=bob)
    call('unknown payment method', 'POST /api/orders/{id}/pay', f"/api/orders/{mixed['id']}/pay", {'method': 'cash'}, alice)
    call('default wallet payment omitted body', 'POST /api/orders/{id}/pay', f"/api/orders/{rounded['id']}/pay", token=alice)
    call('partial refund numeric string cross user retained policy', 'POST /api/orders/{id}/refund', f"/api/orders/{rounded['id']}/refund", {'amount': '1.25'}, bob)
    call('remaining partial refund', 'POST /api/orders/{id}/refund', f"/api/orders/{rounded['id']}/refund", {'amount': rounded['total'] - 1.25}, alice)
    call('cross user cancel retained policy', 'POST /api/orders/{id}/cancel', f"/api/orders/{mixed['id']}/cancel", token=bob)
    settled = order('pending arbitrary settlement order', [{'product_id': products[1]['id']}])
    raw = json.dumps({'order_id': settled['id'], 'bank_ref': 'arbitrary-' + suffix, 'status': 'settled'}, separators=(',', ':')).encode()
    client.call('signed arbitrary bank reference', 'POST /api/webhooks/bank', body=raw,
                headers={'X-Signature': hmac.new(secret.encode(), raw, hashlib.sha256).hexdigest()})
    call('settled order preserves paid amount', 'GET /api/orders/{id}', f"/api/orders/{settled['id']}", token=alice)
    refund = order('full refund order', [{'product_id': products[2]['id']}])
    call('explicit wallet payment', 'POST /api/orders/{id}/pay', f"/api/orders/{refund['id']}/pay", {'method': 'wallet'}, alice)
    call('refund omitted body', 'POST /api/orders/{id}/refund', f"/api/orders/{refund['id']}/refund", token=alice)
    for code in ['SAVE10', 'ONCE50', 'ONCE50', 'NO_SUCH_COUPON']:
        call('seed coupon ' + code + f' #{len(client.observations)}', 'POST /api/orders', body={'items': [{'product_id': pid}], 'coupon': code}, token=alice)
    order('explicit empty coupon', [{'product_id': pid}], coupon='')
    call('orders newest first all rows', 'GET /api/orders', '/api/orders?limit=-1', token=alice)
    call('admin user list', 'GET /api/admin/users', token=admin, policy='password_hash')
    call('default user role omitted body', 'POST /api/admin/users/{id}/role', f"/api/admin/users/{users[1]['id']}/role", token=admin)
    call('admin shipped status', 'POST /api/admin/orders/{id}/status', f"/api/admin/orders/{settled['id']}/status", {'status': 'shipped'}, admin)
    for group in ['day', 'status', 'user']:
        call('sales bucket ' + group, 'GET /api/reports/sales', '/api/reports/sales?group_by=' + group, token=admin)
    call('sales empty exclusive date range', 'GET /api/reports/sales', '/api/reports/sales?from=1970-01-01&to=1970-01-01', token=admin)
    before = ok(call('fresh report baseline', 'GET /api/reports/sales', f'/api/reports/sales?group_by=status&run={suffix}', token=admin))
    extra = order('freshness new order', [{'product_id': products[2]['id']}])
    after = call('fresh report after committed order', 'GET /api/reports/sales', f'/api/reports/sales?group_by=status&run={suffix}', token=admin, policy='current committed report data')
    fresh = copy.deepcopy(before)
    bucket = next((b for b in fresh['buckets'] if b['bucket'] == 'pending'), None)
    if bucket:
        bucket['orders'] += 1
        bucket['revenue'] += extra['total']
    else:
        fresh['buckets'].append({'bucket': 'pending', 'orders': 1, 'revenue': extra['total']})
        fresh['buckets'].sort(key=lambda b: b['bucket'])
    client.known(after, {'status': 200, 'body': before}, {'status': 200, 'body': fresh})
    export = ok(call('export omitted body', 'POST /api/reports/export', token=admin))
    call('download decoded gzip CSV', 'GET /api/reports/download', '/api/reports/download?' + urllib.parse.urlencode(export), token=admin)
    filtered_export = ok(call('export exclusive empty bounds', 'POST /api/reports/export', body={'filename': 'filtered-' + suffix + '.csv', 'from': '1970-01-01', 'to': '1970-01-01'}, token=admin))
    filtered = ok(call('download filtered empty CSV', 'GET /api/reports/download', '/api/reports/download?' + urllib.parse.urlencode(filtered_export), token=admin))
    if len(filtered['csv']) != 1:
        raise ValueError('export ignored exclusive empty date range')
    # These divergent writes run last so they cannot contaminate paired scenarios.
    anonymous = call('anonymous admin denied', 'GET /api/admin/users', policy='admin authentication')
    expected_users = next(o.body for o in client.observations if o.label == 'admin user list')
    # User balances are unchanged since the admin list, except freshness order stock reservation.
    client.known(anonymous, {'status': 200, 'body': expected_users}, {'status': 401, 'body': {'error': 'unauthorized'}})
    call('non-admin denied', 'GET /api/admin/users', token=bob)
    old_profile = ok(call('allowlist initial profile', 'GET /api/users/me', token=profile))
    patch = call('profile ignores username', 'PATCH /api/users/me', body={'username': 'changed_' + suffix}, token=profile, policy='profile name/email allowlist')
    changed = dict(old_profile, username='changed_' + suffix)
    client.known(patch, {'status': 200, 'body': changed}, {'status': 200, 'body': old_profile})
    call('logout', 'POST /api/logout', token=alice)
    logged_out = call('logout token revoked', 'GET /api/users/me', token=alice, policy='logout revokes bearer token')
    # Capture the expected legacy profile after all money operations, before logout.
    expected = dict(next(o.body for o in client.observations if o.label == 'profile email only'))
    expected['wallet_balance'] = 500.125 - 10.25
    client.known(logged_out, {'status': 200, 'body': expected}, {'status': 401, 'body': {'error': 'unauthorized'}})
    method = call('method error JSON contract', 'DELETE /api/products', policy='method errors keep JSON and HTTP status')
    if side == 'legacy' and method.status == 500 and isinstance(method.body.get('trace'), str):
        legacy_method = {'status': 500, 'body': method.body}
        if method.body.get('error') != '405 Method Not Allowed: The method is not allowed for the requested URL.':
            raise ValueError('unexpected legacy method error')
    else:
        legacy_method = {'status': 500, 'body': None}
    client.known(method, legacy_method, {'status': 405, 'body': {'error': 'method not allowed'}})


def self_test():
    baseline = Observation('control', 'GET /api/orders', 200, {'total': 12.5, 'items': [{'id': 1, 'qty': 2}, {'id': 2, 'qty': 3}], 'error': 'original'})
    controls = {}
    changed = copy.deepcopy(baseline); changed.status = 201; controls['status'] = changed
    changed = copy.deepcopy(baseline); changed.body['error'] = 'changed'; controls['error text'] = changed
    changed = copy.deepcopy(baseline); changed.body['total'] = 12.51; controls['money'] = changed
    changed = copy.deepcopy(baseline); changed.body['items'][0]['qty'], changed.body['items'][1]['qty'] = 3, 2; controls['row assignment'] = changed
    changed = copy.deepcopy(baseline); del changed.body['total']; controls['missing key'] = changed
    assert compare(baseline, copy.deepcopy(baseline))[0] == 'match'
    for name, changed in controls.items():
        assert compare(baseline, changed)[0] == 'mismatch', name
    print('self-test: five negative controls caught; identical observation matches')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--legacy')
    parser.add_argument('--rust')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--bank-secret', default='bank-shared-secret-2019')
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not args.legacy or not args.rust:
        parser.error('--legacy and --rust are required unless --self-test is used')
    suffix = uuid.uuid4().hex[:10]
    clients = [Client(args.legacy), Client(args.rust)]
    for client, side in zip(clients, ['legacy', 'rust']):
        try:
            scenario(client, suffix, args.bank_secret, side)
        except (ValueError, KeyError, TypeError, urllib.error.URLError) as error:
            print(f'{side}: comparison inconclusive: {error}', file=sys.stderr)
            return 2
    left, right = clients[0].observations, clients[1].observations
    results = []
    for a, b in zip(left, right):
        verdict, reason = compare(a, b)
        results.append({'label': a.label, 'operation': a.operation, 'verdict': verdict, 'reason': reason,
                        'legacy': comparable(a), 'rust': comparable(b)})
        print(f'{verdict}: {a.label}')
    covered = {o.operation for o in left} & {o.operation for o in right}
    uncovered = sorted(OPERATIONS - covered)
    mismatch = sum(r['verdict'] == 'mismatch' for r in results)
    report = {'suffix': suffix, 'observations': len(results), 'mismatches': mismatch,
              'operation_count': len(OPERATIONS & covered), 'uncovered': uncovered,
              'limitations': ['Coupon expiry dates cannot be changed through the HTTP API.',
                              'Requires isolated identically seeded databases; does not reset them.',
                              'External bank charging is not exercised; signed settlement is exercised.'],
              'results': results}
    if len(left) != len(right):
        report['unpaired'] = [len(left), len(right)]
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(results)} observations; {len(OPERATIONS & covered)}/27 operations; {mismatch} unexplained mismatches; uncovered={uncovered}")
    return int(bool(mismatch or uncovered or len(left) != len(right)))


if __name__ == '__main__':
    sys.exit(main())
