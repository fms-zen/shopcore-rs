"""Focused API regressions; run against the seeded Rust service via BASE_URL."""

from concurrent.futures import ThreadPoolExecutor
import os
import uuid

import requests


BASE = os.environ.get("BASE_URL", "http://localhost:8000")


def api(method, path, token=None, **kwargs):
    headers = kwargs.pop("headers", {})
    if token:
        headers["Authorization"] = "Bearer " + token
    response = requests.request(method, BASE + path, headers=headers, timeout=15, **kwargs)
    if response.status_code >= 400:
        assert isinstance(response.json().get("error"), str), response.text
    return response


def user():
    username = "auth_" + uuid.uuid4().hex
    credentials = {"username": username, "email": username + "@example.com", "password": "pw12345"}
    created = api("POST", "/api/register", json=credentials)
    assert created.status_code == 201, created.text
    logged_in = api("POST", "/api/login", json=credentials)
    assert logged_in.status_code == 200, logged_in.text
    return created.json()["id"], logged_in.json()["token"]


def admin():
    response = api("POST", "/api/login", json={"username": "admin", "password": "admin123"})
    assert response.status_code == 200, response.text
    return response.json()["token"]


def test_admin_authorization_profile_allowlist_and_logout():
    uid, token = user()
    for method, path, body in [
        ("GET", "/api/admin/users", None),
        ("POST", f"/api/admin/users/{uid}/role", {"role": "admin"}),
        ("POST", "/api/admin/orders/99999999/status", {"status": "shipped"}),
        ("POST", "/api/products", {"sku": "forbidden", "name": "forbidden"}),
        ("PUT", "/api/products/99999999", {"name": "forbidden"}),
    ]:
        assert api(method, path, json=body).status_code == 401
        assert api(method, path, token, json=body).status_code == 403
    before = api("GET", "/api/users/me", token).json()
    response = api("PATCH", "/api/users/me", token, json={
        "name": "Renamed", "role": "admin", "wallet_balance": 999999,
        "password_hash": "bad", "name = 'injected' WHERE id > 0 --": "bad",
    })
    assert response.status_code == 200, response.text
    profile = response.json()
    assert profile["name"] == "Renamed"
    assert profile["role"] == "user" and profile["wallet_balance"] == before["wallet_balance"]
    assert "password_hash" not in profile
    assert api("POST", "/api/logout", token).json() == {"ok": True}
    assert api("GET", "/api/users/me", token).status_code == 401
    assert api("POST", "/api/logout", token).status_code == 401


def test_registration_duplicates_are_atomic_and_email_update_is_unique():
    username = "race_" + uuid.uuid4().hex
    credentials = {"username": username, "email": username + "@example.com", "password": "pw"}
    with ThreadPoolExecutor(max_workers=8) as executor:
        responses = list(executor.map(lambda _: api("POST", "/api/register", json=credentials), range(8)))
    assert sorted(response.status_code for response in responses) == [201] + [409] * 7
    _, token = user()
    response = api("PATCH", "/api/users/me", token, json={"email": credentials["email"]})
    assert response.status_code == 409, response.text
    # Sharing an email is rejected even when concurrent usernames differ.
    shared_email = "email_race_" + uuid.uuid4().hex + "@example.com"
    with ThreadPoolExecutor(max_workers=6) as executor:
        responses = list(executor.map(lambda _: api("POST", "/api/register", json={
            "username": "mail_" + uuid.uuid4().hex, "email": shared_email, "password": "pw",
        }), range(6)))
    assert sorted(response.status_code for response in responses) == [201] + [409] * 5


def test_catalog_sort_search_partial_update_and_identifier_rejection():
    token = admin()
    marker = "Catalog-" + uuid.uuid4().hex
    products = []
    for index, amount in enumerate([23.5, 4.25, 13.0]):
        response = api("POST", "/api/products", token, json={
            "sku": f"{marker}-{index}", "name": f"{marker} Widget {index}", "price": amount, "stock": 10,
        })
        assert response.status_code == 201, response.text
        products.append(response.json())
    response = api("GET", "/api/products", params={"q": marker.lower(), "sort": "price", "limit": -1})
    assert response.status_code == 200, response.text
    assert [p["price"] for p in response.json()] == [4.25, 13.0, 23.5]
    response = api("PUT", f"/api/products/{products[0]['id']}", token, json={"price": 25.0})
    assert response.status_code == 200, response.text
    assert response.json()["sku"] == products[0]["sku"] and response.json()["stock"] == 10
    assert response.json()["price"] == 25.0
    assert api("GET", "/api/products", params={"sort": "price; DROP TABLE products"}).status_code == 400
    assert api("POST", "/api/products", token, json={"price": "NaN"}).status_code == 400
    assert api("POST", "/api/products", token, json={"price": -1}).status_code == 400
    assert "password_hash" not in api("GET", "/api/admin/users", token).json()[0]


def test_malformed_bodies_and_unknown_resources_have_json_errors():
    assert api("POST", "/api/login").status_code == 401
    token = admin()
    response = api("POST", "/api/products", token)
    assert response.status_code == 201, response.text
    product = response.json()
    assert product["price"] == 0 and product["stock"] == 0
    response = api("PUT", f"/api/products/{product['id']}", token)
    assert response.status_code == 200 and response.json() == product
    assert api("POST", "/api/register", data="{", headers={"Content-Type": "application/json"}).status_code == 400
    assert api("POST", "/api/register", json=[]).status_code == 400
    assert api("POST", "/api/register", json={"username": "missing-password"}).status_code == 400
    assert api("GET", "/api/products/99999999").status_code == 404
