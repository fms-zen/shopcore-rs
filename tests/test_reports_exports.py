"""Focused report compatibility checks against the running Rust HTTP service."""
import csv
import gzip
import io
import uuid
from datetime import datetime, timedelta

import pytest

from test_smoke import admin_token, api, new_user

WIDE = {"from": "2000-01-01", "to": "2999-12-31"}


def checked(method, path, token, status=200, **kwargs):
    response = api(method, path, token, **kwargs)
    assert response.status_code == status, response.text
    return response


def product(admin, name, price=987654.25):
    return checked("POST", "/api/products", admin, 201, json={
        "sku": "REPORT-" + uuid.uuid4().hex, "name": name, "price": price, "stock": 100,
    }).json()


def order(user, item, quantity=1, **extra):
    return checked("POST", "/api/orders", user["token"], 201, json={
        "items": [{"product_id": item["id"], "quantity": quantity}], **extra,
    }).json()


def report(admin, **params):
    return checked("GET", "/api/reports/sales", admin, params={**WIDE, **params}).json()


def test_reports_refresh_group_all_statuses_and_rank_gross_product_revenue():
    admin, user = admin_token(), new_user("report")
    name = "Shared report product " + uuid.uuid4().hex
    first, second = product(admin, name), product(admin, name)
    before = report(admin, group_by="user")
    assert not any(row["bucket"] == str(user["id"]) for row in before["buckets"])
    pending = order(user, first)
    paid = order(user, first, 2, coupon="SAVE10")
    checked("POST", "/api/wallet/topup", user["token"], json={"amount": paid["total"]})
    checked("POST", f'/api/orders/{paid["id"]}/pay', user["token"], json={"method": "wallet"})
    shipped = order(user, second, 3)
    checked("POST", f'/api/admin/orders/{shipped["id"]}/status', admin, json={"status": "shipped"})
    cancelled = order(user, first)
    checked("POST", f'/api/orders/{cancelled["id"]}/cancel', user["token"])

    after = report(admin, group_by="user")  # Exactly the same URL as before.
    bucket = next(row for row in after["buckets"] if row["bucket"] == str(user["id"]))
    assert bucket["orders"] == 4
    assert bucket["revenue"] == pytest.approx(sum(o["total"] for o in [pending, paid, shipped, cancelled]))
    top = next(row for row in after["top_products"] if row["name"] == name)
    assert top["revenue"] == pytest.approx(first["price"] * 5)  # No coupon allocation.
    statuses = {row["bucket"] for row in report(admin, group_by="status")["buckets"]}
    assert {"pending", "paid", "shipped", "cancelled"} <= statuses
    assert all(len(row["bucket"]) == 10 for row in report(admin, group_by="day")["buckets"])
    checked("GET", "/api/reports/sales", admin, 400, params={"group_by": "user_id; SELECT 1"})
    checked("GET", "/api/reports/sales", None, 401)
    checked("GET", "/api/reports/sales", user["token"], 403)


def test_http_export_is_escaped_gzip_and_date_range_is_half_open():
    admin, user = admin_token(), new_user("csv")
    name = 'CSV comma, quote " and\nnewline ' + uuid.uuid4().hex
    item = product(admin, name, 12.5)
    created = order(user, item, 2)
    filename = "report_" + uuid.uuid4().hex + ".csv"
    result = checked("POST", "/api/reports/export", admin, json={"filename": filename}).json()
    assert result == {"file": filename + ".gz"}
    download = checked("GET", "/api/reports/download", admin, params=result)
    assert download.headers["Content-Type"].split(";")[0] == "application/gzip"
    records = list(csv.reader(io.StringIO(gzip.decompress(download.content).decode())))
    assert records[0] == ["order_no", "date", "status", "user", "product", "qty", "price"]
    row = next(row for row in records[1:] if row[0] == str(created["order_no"]))
    assert row[2:6] == ["pending", user["username"], name, "2"]
    assert float(row[6]) == 12.5

    exact = row[1]
    empty = report(admin, group_by="user", **{"from": exact, "to": exact})
    assert empty["buckets"] == [] and empty["top_products"] == []
    end = (datetime.fromisoformat(exact) + timedelta(microseconds=1)).isoformat(" ")
    included = report(admin, group_by="user", **{"from": exact, "to": end})
    assert any(row["bucket"] == str(user["id"]) and row["orders"] == 1 for row in included["buckets"])
    checked("GET", "/api/reports/sales", admin, 400, params={"from": "not-a-date"})
    default = checked("POST", "/api/reports/export", admin, json={}).json()
    assert default == {"file": "sales.csv.gz"}


def test_report_export_and_download_reject_paths():
    admin = admin_token()
    for path in ["../outside.csv", "/tmp/outside.csv", "nested/file.csv", "nested\\file.csv"]:
        checked("POST", "/api/reports/export", admin, 400, json={"filename": path})
        response = api("GET", "/api/reports/download", admin, params={"file": path})
        assert response.status_code in (400, 404), response.text
        assert isinstance(response.json()["error"], str)
    checked("GET", "/api/reports/download", admin, 404, params={"file": "absent_" + uuid.uuid4().hex})
