"""Money invariants against a running app and bank simulator (run serially)."""

from concurrent.futures import ThreadPoolExecutor
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, HTTPServer
import ipaddress
import json
import os
import queue
import socket
import threading
from urllib.parse import urlparse
import uuid

import pytest
import requests


BASE = os.environ.get("BASE_URL", "http://localhost:8000")
BANK = os.environ.get("BANK_URL", "http://localhost:5001")
SECRET = os.environ.get("BANK_SECRET", "bank-shared-secret-2019").encode()


def api(method, path, token=None, **kwargs):
    headers = dict(kwargs.pop("headers", {}))
    if token:
        headers["Authorization"] = "Bearer " + token
    return requests.request(method, BASE + path, headers=headers, timeout=15, **kwargs)


def user():
    name = "money_" + uuid.uuid4().hex
    response = api("POST", "/api/register", json={
        "username": name, "email": name + "@example.com", "name": name, "password": "test-password"
    })
    assert response.status_code == 201, response.text
    token = api("POST", "/api/login", json={"username": name, "password": "test-password"})
    assert token.status_code == 200, token.text
    return {"id": response.json()["id"], "token": token.json()["token"]}


def balance(owner):
    response = api("GET", "/api/wallet", owner["token"])
    assert response.status_code == 200, response.text
    return response.json()["balance"]


def topup(owner, amount):
    response = api("POST", "/api/wallet/topup", owner["token"], json={"amount": amount})
    assert response.status_code == 200, response.text
    return response.json()["balance"]


def order(owner, notify_url=None):
    products = api("GET", "/api/products?sort=id&limit=1")
    assert products.status_code == 200, products.text
    response = api("POST", "/api/orders", owner["token"], json={
        "items": [{"product_id": products.json()[0]["id"], "quantity": 1}],
        **({"notify_url": notify_url} if notify_url else {}),
    })
    assert response.status_code == 201, response.text
    return response.json()


def callback_host():
    """Choose the listener's address reachable from the app's network."""
    app = urlparse(BASE)
    app_port = app.port or (443 if app.scheme == "https" else 80)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
        route.connect((app.hostname, app_port))
        address = route.getsockname()[0]
        if ipaddress.ip_address(address).is_loopback:
            # For a host-run test against localhost's published Docker port,
            # advertise the host's routed address instead of container loopback.
            # UDP connect selects a route without transmitting any packets.
            route.connect(("8.8.8.8", 80))
            address = route.getsockname()[0]
    return address


def test_pending_and_paid_order_notifications_reach_http_receiver():
    owner = user()
    callbacks = queue.Queue()

    class Receiver(BaseHTTPRequestHandler):
        def do_POST(self):
            self.connection.settimeout(5)
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")
            callbacks.put(payload)

        def log_message(self, *args):
            pass

    server = HTTPServer(("0.0.0.0", 0), Receiver)
    thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.1), daemon=True)
    thread.start()
    purchase = None
    try:
        notify_url = "http://%s:%s/notify" % (callback_host(), server.server_port)
        purchase = order(owner, notify_url=notify_url)
        expected = {"order_id": purchase["id"], "order_no": purchase["order_no"], "status": "pending"}
        assert callbacks.get(timeout=10) == expected
        topup(owner, purchase["total"])
        response = api("POST", "/api/orders/%s/pay" % purchase["id"], owner["token"], json={"method": "wallet"})
        assert response.status_code == 200, response.text
        assert callbacks.get(timeout=10) == {**expected, "status": "paid"}
    finally:
        try:
            if purchase is not None:
                api("POST", "/api/orders/%s/cancel" % purchase["id"], owner["token"])
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def test_concurrent_wallet_topups_transfers_and_self_transfer_conserve_money():
    source, target = user(), user()
    with ThreadPoolExecutor(max_workers=10) as pool:
        results = list(pool.map(lambda _: topup(source, 5), range(20)))
    assert max(results) == 100
    assert balance(source) == 100
    with ThreadPoolExecutor(max_workers=10) as pool:
        responses = list(pool.map(lambda _: api("POST", "/api/wallet/transfer", source["token"],
            json={"to_user_id": target["id"], "amount": 10}), range(20)))
    assert all(response.status_code in (200, 402) for response in responses)
    assert sum(response.status_code == 200 for response in responses) == 10
    assert balance(source) == 0
    assert balance(target) == 100
    with ThreadPoolExecutor(max_workers=10) as pool:
        responses = list(pool.map(lambda _: api("POST", "/api/wallet/transfer", target["token"],
            json={"to_user_id": target["id"], "amount": 10}), range(20)))
    assert all(response.status_code == 200 for response in responses)
    assert balance(target) == 100


def test_opposing_transfers_finish_without_deadlock_and_conserve_money():
    first, second = user(), user()
    topup(first, 100)
    topup(second, 100)
    def transfer(index):
        source, target = (first, second) if index % 2 else (second, first)
        return api("POST", "/api/wallet/transfer", source["token"],
            json={"to_user_id": target["id"], "amount": 1})
    with ThreadPoolExecutor(max_workers=10) as pool:
        responses = list(pool.map(transfer, range(40)))
    assert all(response.status_code == 200 for response in responses)
    assert balance(first) == balance(second) == 100


def test_simultaneous_order_payments_debit_once():
    owner = user()
    purchase = order(owner)
    initial = purchase["total"] * 5 + 100
    topup(owner, initial)
    with ThreadPoolExecutor(max_workers=8) as pool:
        responses = list(pool.map(lambda _: api("POST", "/api/orders/%s/pay" % purchase["id"],
            owner["token"], json={"method": "wallet"}), range(8)))
    assert sum(response.status_code == 200 for response in responses) == 1
    assert sum(response.status_code == 409 for response in responses) == 7
    success = next(response for response in responses if response.status_code == 200)
    assert success.json()["bank_ref"] is None
    assert balance(owner) == pytest.approx(initial - purchase["total"])


def test_payments_and_transfer_compete_for_the_same_wallet_funds():
    owner, recipient = user(), user()
    purchases = [order(owner), order(owner)]
    total = purchases[0]["total"]
    assert total > 0 and purchases[1]["total"] == total
    initial = total * 1.5
    topup(owner, initial)
    operations = [
        ("/api/orders/%s/pay" % purchase["id"], {"method": "wallet"})
        for purchase in purchases
    ] + [("/api/wallet/transfer", {"to_user_id": recipient["id"], "amount": total})]
    with ThreadPoolExecutor(max_workers=3) as pool:
        responses = list(pool.map(lambda operation: api("POST", operation[0], owner["token"], json=operation[1]), operations))
    assert sum(response.status_code == 200 for response in responses) == 1
    assert sum(response.status_code == 402 for response in responses) == 2
    paid = sum(response.status_code == 200 for response in responses[:2]) * total
    assert balance(owner) >= 0
    assert balance(owner) + balance(recipient) + paid == pytest.approx(initial)


def test_bank_retry_uses_one_logical_charge():
    # This test alone changes simulator settings, and always restores them. Do
    # not reset the simulator, because its charge history may belong to others.
    owner = user()
    purchase = order(owner)
    original = requests.get(BANK + "/config", timeout=5)
    assert original.status_code == 200, original.text
    original = original.json()
    before = requests.get(BANK + "/stats", timeout=5).json()["charges"]
    try:
        configured = requests.post(BANK + "/config", json={"latency_ms": 0, "drop_rate": 1}, timeout=5)
        assert configured.status_code == 200, configured.text
        # The first simulator request records a charge then returns 503. Only a
        # stable key lets the retry recover the original reference with HTTP200.
        response = api("POST", "/api/orders/%s/pay" % purchase["id"], owner["token"], json={"method": "bank"})
        assert response.status_code == 200, response.text
        stats = requests.get(BANK + "/stats", timeout=5).json()
        charges = [item for item in stats["items"] if item["order_id"] == purchase["id"]]
        assert stats["charges"] - before == 1
        assert len(charges) == 1
        assert charges[0]["ref"] == response.json()["bank_ref"]
        repeated = api("POST", "/api/orders/%s/pay" % purchase["id"], owner["token"], json={"method": "bank"})
        assert repeated.status_code == 409
        assert requests.get(BANK + "/stats", timeout=5).json()["charges"] == stats["charges"]
    finally:
        restored = requests.post(BANK + "/config", json=original, timeout=5)
        assert restored.status_code == 200, restored.text


def test_raw_webhook_signature_and_duplicate_settlement():
    owner = user()
    purchase = order(owner)
    raw = json.dumps({"order_id": purchase["id"], "bank_ref": "arbitrary-ref", "status": "settled"}, indent=2).encode()
    signature = hmac.new(SECRET, raw, hashlib.sha256).hexdigest()
    for headers in ({}, {"X-Signature": "deadbeef"}):
        rejected = api("POST", "/api/webhooks/bank", data=raw, headers={"Content-Type": "application/json", **headers})
        assert rejected.status_code == 401, rejected.text
    compact = json.dumps(json.loads(raw), separators=(",", ":")).encode()
    rejected = api("POST", "/api/webhooks/bank", data=compact,
        headers={"Content-Type": "application/json", "X-Signature": signature})
    assert rejected.status_code == 401
    for _ in range(2):
        response = api("POST", "/api/webhooks/bank", data=raw,
            headers={"Content-Type": "application/json", "X-Signature": signature})
        assert response.status_code == 200, response.text
        assert response.json() == {"ok": True}
    result = api("GET", "/api/orders/%s" % purchase["id"], owner["token"]).json()
    assert result["status"] == "paid"
    assert result["paid_amount"] == 0
    assert balance(owner) == 0


@pytest.mark.parametrize("amount", [0, -1, "NaN", "Infinity", "bad"])
def test_invalid_wallet_amounts_cannot_create_credit(amount):
    source, target = user(), user()
    topup(source, 10)
    for path, payload in (("/api/wallet/topup", {"amount": amount}),
            ("/api/wallet/transfer", {"to_user_id": target["id"], "amount": amount})):
        response = api("POST", path, source["token"], json=payload)
        assert response.status_code == 400, response.text
    assert balance(source) == 10
    assert balance(target) == 0
