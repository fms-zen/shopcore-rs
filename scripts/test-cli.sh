#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

cli() {
    docker compose exec -T app python manage.py "$@"
}

sql() {
    docker compose exec -T db sh -c \
        'exec psql -X -qAt -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" "$@"' sh "$@"
}

assert_equal() {
    if [ "$1" != "$2" ]; then
        printf 'CLI acceptance failed: expected <%s>, got <%s>\n' "$2" "$1" >&2
        exit 1
    fi
}

assert_cli() {
    expected=$1
    shift
    output=$(cli "$@")
    assert_equal "$output" "$expected"
}

fixture=$(docker compose exec -T app python -c 'import uuid; print("cli_" + uuid.uuid4().hex)')
fixture_dir=/tmp/$fixture
cleanup() {
    sql -c "BEGIN;
        DELETE FROM orders WHERE user_id IN (SELECT id FROM users WHERE username = '$fixture');
        DELETE FROM users WHERE username = '$fixture';
        DELETE FROM products WHERE sku IN ('$fixture', '$fixture-rollback', '$fixture-invalid');
        COMMIT;" >/dev/null || return
    docker compose exec -T app rm -rf "$fixture_dir"
}
trap 'status=$?; trap - 0; cleanup || status=1; exit "$status"' 0
trap 'exit 130' INT
trap 'exit 143' TERM

snapshot='SELECT md5(json_build_array(
    (SELECT json_agg(t ORDER BY id) FROM users t),
    (SELECT json_agg(t ORDER BY id) FROM products t),
    (SELECT json_agg(t ORDER BY id) FROM coupons t),
    (SELECT json_agg(t ORDER BY id) FROM orders t),
    (SELECT json_agg(t ORDER BY id) FROM order_items t),
    (SELECT json_agg(t ORDER BY id) FROM payments t))::text)'
before=$(sql -c "$snapshot")
assert_cli 'tables ready' init-db
assert_cli 'tables ready' init-db
assert_equal "$(sql -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('users','products','coupons','orders','order_items','payments')")" 6
assert_cli seeded seed
assert_cli seeded seed
assert_equal "$(sql -c "$snapshot")" "$before"
sql -c "SELECT json_build_object(
    'products', (SELECT json_agg(json_build_array(sku,name,price) ORDER BY sku) FROM products WHERE sku BETWEEN 'SKU-1001' AND 'SKU-1010'),
    'coupons', (SELECT json_agg(json_build_array(code,percent,expires,single_use) ORDER BY code) FROM coupons WHERE code IN ('SAVE10','ONCE50')),
    'users', (SELECT json_agg(json_build_array(username,email,name,password_hash,role)) FROM users))" |
docker compose exec -T app python -c '
import csv, hashlib, json, os, sys
seeded = json.load(sys.stdin)
with open("seed/products.csv", newline="") as source:
    products = [[row["sku"], row["name"], float(row["price"])] for row in csv.DictReader(source)]
assert seeded["products"] == products, seeded["products"]
assert seeded["coupons"] == [["ONCE50", 50, "2099-12-31", True], ["SAVE10", 10, "2099-12-31", False]], seeded["coupons"]
username = os.environ.get("ADMIN_USER", "admin")
password_hash = hashlib.md5(os.environ.get("ADMIN_PASSWORD", "admin123").encode()).hexdigest()
admin = [user for user in seeded["users"] if user[0] == username]
assert admin == [[username, "admin@shopcore.local", "Administrator", password_hash, "admin"]], admin
'

docker compose exec -T app mkdir "$fixture_dir"
docker compose exec -T app sh -c 'cat > "$1/import.csv"' sh "$fixture_dir" <<EOF
sku,name,price,stock
$fixture,"CLI, ""quoted"" product",12.5,7
$fixture,ignored,invalid,invalid
EOF
assert_cli 'imported 1 products' import-products "$fixture_dir/import.csv"
assert_equal "$(sql -c "SELECT count(*), min(name), min(price), min(stock) FROM products WHERE sku = '$fixture'")" \
    '1|CLI, "quoted" product|12.5|7'
assert_cli 'imported 1 products' import-products "$fixture_dir/import.csv"
assert_equal "$(sql -c "SELECT count(*), count(DISTINCT (name,price,stock)) FROM products WHERE sku = '$fixture'")" '2|1'

docker compose exec -T app sh -c 'cat > "$1/invalid.csv"' sh "$fixture_dir" <<EOF
sku,name,price,stock
$fixture-rollback,Must roll back,5,2
$fixture-invalid,Invalid second row,invalid,2
EOF
if output=$(cli import-products "$fixture_dir/invalid.csv" 2>&1); then
    printf 'CLI acceptance failed: invalid import succeeded\n' >&2
    exit 1
fi
printf '%s\n' "$output" | grep -F 'invalid product price' >/dev/null || {
    printf 'CLI acceptance failed: unexpected import error <%s>\n' "$output" >&2
    exit 1
}
assert_equal "$(sql -c "SELECT count(*) FROM products WHERE sku IN ('$fixture-rollback', '$fixture-invalid')")" 0

order_no=$(sql -c "WITH fixture_user AS (
    INSERT INTO users (username,email,password_hash) VALUES ('$fixture','$fixture@example.test',md5('cli-test-password')) RETURNING id
    ) INSERT INTO orders (order_no,user_id,status,total,created_at)
    SELECT -id,id,'cancelled',25,'2001-02-03 04:05:06' FROM fixture_user RETURNING order_no")
order_count=$(sql -c 'SELECT count(*) FROM orders')
assert_cli "wrote $fixture_dir/orders.csv ($order_count rows)" export-sales "$fixture_dir/orders.csv"
docker compose exec -T app python - "$fixture_dir/orders.csv" "$order_no" "$order_count" <<'PY'
import csv
import sys

path, order_no, count = sys.argv[1:]
with open(path, newline="") as exported:
    rows = list(csv.reader(exported))
assert rows[0] == ["order_no", "status", "total", "created_at"], rows[0]
assert len(rows) == int(count) + 1, len(rows)
expected = [order_no, "cancelled", "25.0", "2001-02-03 04:05:06"]
assert rows[1:].count(expected) == 1, (expected, rows)
assert all(len(row) == 4 for row in rows), rows
PY

docker compose exec -T app python - "$fixture" "$fixture_dir/token" <<'PY'
import json
from pathlib import Path
import sys
import urllib.request

username, token_path = sys.argv[1:]
request = urllib.request.Request(
    "http://localhost:8000/api/login",
    data=json.dumps({"username": username, "password": "cli-test-password"}).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(request, timeout=10) as response:
    token = json.load(response)["token"]
Path(token_path).write_text(token)
request = urllib.request.Request("http://localhost:8000/api/users/me", headers={"Authorization": "Bearer " + token})
with urllib.request.urlopen(request, timeout=10) as response:
    assert json.load(response)["username"] == username
PY
assert_cli 'purged sessions' purge-sessions
docker compose exec -T app python - "$fixture" "$fixture_dir/token" <<'PY'
import json
from pathlib import Path
import sys
import urllib.request

username, token_path = sys.argv[1:]
headers = {"Authorization": "Bearer " + Path(token_path).read_text()}
request = urllib.request.Request("http://localhost:8000/api/users/me", headers=headers)
with urllib.request.urlopen(request, timeout=10) as response:
    assert json.load(response)["username"] == username
request = urllib.request.Request("http://localhost:8000/api/logout", data=b"", headers=headers)
with urllib.request.urlopen(request, timeout=10) as response:
    assert json.load(response) == {"ok": True}
PY

printf 'CLI acceptance passed\n'
