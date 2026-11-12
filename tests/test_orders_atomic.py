"""Order rollback and contention checks using isolated users, products and coupons."""

from concurrent.futures import ThreadPoolExecutor
from datetime import date, timedelta
import os
from threading import Barrier
import uuid

import pytest
import requests


BASE = os.environ.get("BASE_URL", "http://localhost:8000")


def api(method, path, token=None, **kwargs):
    headers = kwargs.pop("headers", {})
    if token:
        headers["Authorization"] = "Bearer " + token
    return requests.request(method, BASE + path, headers=headers, timeout=20, **kwargs)


@pytest.fixture
def admin():
    result = api("POST", "/api/login", json={"username": "admin", "password": "admin123"})
    assert result.status_code == 200, result.text
    return result.json()["token"]


@pytest.fixture
def user():
    username = "atomic_" + uuid.uuid4().hex
    result = api("POST", "/api/register", json={
        "username": username, "email": username + "@example.com", "password": "pw12345", "name": username,
    })
    assert result.status_code == 201, result.text
    result = api("POST", "/api/login", json={"username": username, "password": "pw12345"})
    assert result.status_code == 200, result.text
    return result.json()["token"]


@pytest.fixture
def product(admin):
    def create(stock=5, price=10):
        result = api("POST", "/api/products", admin, json={
            "sku": "AT-" + uuid.uuid4().hex, "name": "Atomic test product", "price": price, "stock": stock,
        })
        assert result.status_code == 201, result.text
        return result.json()
    return create


def concurrently(*calls):
    barrier = Barrier(len(calls))

    def run(call):
        barrier.wait(timeout=10)
        return call()

    with ThreadPoolExecutor(max_workers=len(calls)) as executor:
        return list(executor.map(run, calls))


def stock(product_id):
    result = api("GET", f"/api/products/{product_id}")
    assert result.status_code == 200, result.text
    return result.json()["stock"]


def make_order(user, product_id, quantity=1, **extra):
    return api("POST", "/api/orders", user, json={
        "items": [{"product_id": product_id, "quantity": quantity}], **extra,
    })


@pytest.mark.parametrize("coupon", [{}, {"coupon": None}, {"coupon": ""}], ids=["omitted", "null", "empty"])
def test_no_coupon_preserves_representation_without_discount(user, product, coupon):
    item = product(stock=3, price=10)
    created = make_order(user, item["id"], **coupon)
    assert created.status_code == 201, created.text
    order_id = created.json()["id"]
    detail = api("GET", f"/api/orders/{order_id}", user)
    assert detail.status_code == 200, detail.text
    listed = api("GET", "/api/orders?limit=-1", user)
    assert listed.status_code == 200, listed.text
    assert len(listed.json()) == 1
    for order in [created.json(), detail.json(), listed.json()[0]]:
        assert order["coupon"] == coupon.get("coupon")
        assert order["total"] == 10
    assert stock(item["id"]) == 2


def test_expired_coupon_does_not_change_order_stock_or_usage(user, product):
    import psycopg2

    database_url = os.environ.get("DATABASE_URL")
    if not database_url:
        pytest.skip("DATABASE_URL is required to seed an isolated coupon")
    code = "AT-" + uuid.uuid4().hex
    with psycopg2.connect(database_url) as connection:
        with connection.cursor() as cursor:
            cursor.execute(
                "INSERT INTO coupons (code,percent,expires,single_use,used_count) VALUES (%s,50,'2000-01-01',TRUE,0)",
                (code,),
            )
    try:
        item = product(stock=3)
        before = api("GET", "/api/orders?limit=-1", user).json()
        result = make_order(user, item["id"], coupon=code)
        assert result.status_code == 400, result.text
        assert result.json()["error"] == "coupon expired"
        assert stock(item["id"]) == 3
        assert api("GET", "/api/orders?limit=-1", user).json() == before
        with psycopg2.connect(database_url) as connection:
            with connection.cursor() as cursor:
                cursor.execute("SELECT used_count FROM coupons WHERE code=%s", (code,))
                assert cursor.fetchone()[0] == 0
    finally:
        with psycopg2.connect(database_url) as connection:
            with connection.cursor() as cursor:
                cursor.execute("DELETE FROM coupons WHERE code=%s", (code,))


def test_invalid_later_item_rolls_back_order_and_stock(user, product):
    item = product(stock=5)
    before = api("GET", "/api/orders?limit=-1", user).json()
    result = api("POST", "/api/orders", user, json={"items": [
        {"product_id": item["id"], "quantity": 2},
        {"product_id": 2147483647, "quantity": 1},
    ], "coupon": "SAVE10"})
    assert result.status_code == 404, result.text
    assert stock(item["id"]) == 5
    assert api("GET", "/api/orders?limit=-1", user).json() == before


def test_duplicate_items_cannot_overreserve_and_are_preserved(user, product):
    item = product(stock=3)
    result = api("POST", "/api/orders", user, json={"items": [
        {"product_id": item["id"], "quantity": 2},
        {"product_id": item["id"], "quantity": 2},
    ]})
    assert result.status_code == 409, result.text
    assert stock(item["id"]) == 3
    result = api("POST", "/api/orders", user, json={"items": [
        {"product_id": item["id"]},
        {"product_id": item["id"], "quantity": 2},
    ]})
    assert result.status_code == 201, result.text
    assert [line["quantity"] for line in result.json()["items"]] == [1, 2]
    assert stock(item["id"]) == 0


def test_concurrent_stock_exhaustion_has_one_winner(user, product):
    item = product(stock=1)
    call = lambda: make_order(user, item["id"])
    results = concurrently(call, call)
    assert sorted(result.status_code for result in results) == [201, 409], [r.text for r in results]
    assert stock(item["id"]) == 0
    assert len(api("GET", "/api/orders?limit=-1", user).json()) == 1


def test_single_use_coupon_is_consumed_once(user, product):
    import psycopg2

    database_url = os.environ.get("DATABASE_URL")
    if not database_url:
        pytest.skip("DATABASE_URL is required to seed an isolated coupon")
    code = "AT-" + uuid.uuid4().hex
    with psycopg2.connect(database_url) as connection:
        with connection.cursor() as cursor:
            cursor.execute(
                "INSERT INTO coupons (code,percent,expires,single_use,used_count) VALUES (%s,50,%s,TRUE,0)",
                (code, (date.today() + timedelta(days=10)).isoformat()),
            )
    try:
        item = product(stock=2)
        failed = make_order(user, 2147483647, coupon=code)
        assert failed.status_code == 404, failed.text
        with psycopg2.connect(database_url) as connection:
            with connection.cursor() as cursor:
                cursor.execute("SELECT used_count FROM coupons WHERE code=%s", (code,))
                assert cursor.fetchone()[0] == 0
        call = lambda: make_order(user, item["id"], coupon=code)
        results = concurrently(call, call)
        assert sorted(result.status_code for result in results) == [201, 409], [r.text for r in results]
        assert stock(item["id"]) == 1
        assert next(r.json()["total"] for r in results if r.status_code == 201) == 5
        with psycopg2.connect(database_url) as connection:
            with connection.cursor() as cursor:
                cursor.execute("SELECT used_count FROM coupons WHERE code=%s", (code,))
                assert cursor.fetchone()[0] == 1
    finally:
        with psycopg2.connect(database_url) as connection:
            with connection.cursor() as cursor:
                cursor.execute("DELETE FROM coupons WHERE code=%s", (code,))


def test_concurrent_cancel_restocks_and_credits_once(user, product, admin):
    item = product(stock=3)
    created = make_order(user, item["id"], quantity=2)
    assert created.status_code == 201, created.text
    order_id = created.json()["id"]
    assert api("POST", "/api/wallet/topup", user, json={"amount": 100}).status_code == 200
    paid = api("POST", f"/api/orders/{order_id}/pay", user, json={"method": "wallet"})
    assert paid.status_code == 200, paid.text
    call = lambda: api("POST", f"/api/orders/{order_id}/cancel", user)
    results = concurrently(call, call)
    assert sorted(result.status_code for result in results) == [200, 409], [r.text for r in results]
    assert stock(item["id"]) == 3
    assert api("GET", "/api/wallet", user).json()["balance"] == 100
    assert call().status_code == 409
    reopened = api("POST", f"/api/admin/orders/{order_id}/status", admin, json={"status": "paid"})
    assert reopened.status_code == 409, reopened.text
    assert stock(item["id"]) == 3
    assert api("GET", "/api/wallet", user).json()["balance"] == 100


def test_partial_refunds_stop_at_total_and_cancelled_orders_cannot_refund(user, product):
    item = product(stock=2)
    order = make_order(user, item["id"]).json()
    order_id = order["id"]
    assert api("POST", "/api/wallet/topup", user, json={"amount": 100}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/pay", user, json={"method": "wallet"}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 4}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 7}).status_code == 409
    result = api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 6})
    assert result.status_code == 200, result.text
    assert result.json()["refunded"] == 10
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 1}).status_code == 409
    assert api("GET", "/api/wallet", user).json()["balance"] == 100
    assert api("POST", f"/api/orders/{order_id}/cancel", user).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={}).status_code == 409
    assert api("GET", "/api/wallet", user).json()["balance"] == 100
    assert stock(item["id"]) == 2


def test_cancel_after_partial_refund_and_admin_status_change_credits_only_remainder(user, product, admin):
    item = product(stock=1)
    order_id = make_order(user, item["id"]).json()["id"]
    assert api("POST", "/api/wallet/topup", user, json={"amount": 100}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/pay", user, json={"method": "wallet"}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 4}).status_code == 200
    assert api("POST", f"/api/admin/orders/{order_id}/status", admin, json={"status": "shipped"}).status_code == 200
    cancelled = api("POST", f"/api/orders/{order_id}/cancel", user)
    assert cancelled.status_code == 200, cancelled.text
    assert cancelled.json()["refunded"] == 4
    assert api("GET", "/api/wallet", user).json()["balance"] == 100
    assert stock(item["id"]) == 1


def test_concurrent_refunds_cannot_exceed_total(user, product):
    item = product(stock=1)
    order_id = make_order(user, item["id"]).json()["id"]
    assert api("POST", "/api/wallet/topup", user, json={"amount": 100}).status_code == 200
    assert api("POST", f"/api/orders/{order_id}/pay", user, json={"method": "wallet"}).status_code == 200
    call = lambda: api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 6})
    results = concurrently(call, call)
    assert sorted(r.status_code for r in results) == [200, 409], [r.text for r in results]
    assert api("GET", "/api/wallet", user).json()["balance"] == 96
    assert api("POST", f"/api/orders/{order_id}/refund", user, json={"amount": 4}).status_code == 200
    assert api("GET", "/api/wallet", user).json()["balance"] == 100


@pytest.mark.parametrize("quantity", [0, -1, 1.5, None, "nonsense"])
def test_invalid_quantity_does_not_change_stock(user, product, quantity):
    item = product(stock=3)
    result = make_order(user, item["id"], quantity=quantity)
    assert result.status_code == 400, result.text
    assert stock(item["id"]) == 3
