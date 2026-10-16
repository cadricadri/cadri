#!/bin/sh
# Write /etc/nginx/conf.d/default.conf from CADRI_* environment variables.
# Local Docker leaves CADRI_HTTPS_REDIRECT unset and serves plain HTTP on 8080.
# The cluster sets CADRI_HTTPS_REDIRECT=1. Port 8080 then answers the Let's
# Encrypt HTTP-01 challenge and redirects everything else. Port 8443 terminates
# TLS once /etc/cadri/tls/tls.crt exists (cert-manager fills it after DNS points
# at the NLB, and the entrypoint reloads nginx).
set -eu

upstream=${CADRI_ENGINE_UPSTREAM:-http://engine:8080}
acme=${CADRI_ACME_UPSTREAM:-}
redirect=${CADRI_HTTPS_REDIRECT:-}
cert=/etc/cadri/tls/tls.crt
key=/etc/cadri/tls/tls.key

validate_url() {
    case "$1" in
        http://*|https://*) ;;
        *)
            echo "cadri nginx: URL must start with http:// or https://: $1" >&2
            exit 1
            ;;
    esac
    case "$1" in
        *[!A-Za-z0-9:/._-]*)
            echo "cadri nginx: URL has unexpected characters: $1" >&2
            exit 1
            ;;
    esac
}

validate_url "$upstream"
if [ "$redirect" = "1" ]; then
    validate_url "$acme"
fi

tls=0
if [ -s "$cert" ] && [ -s "$key" ]; then
    tls=1
fi

mkdir -p /etc/nginx/conf.d
tmp=/etc/nginx/conf.d/default.conf.new

security_headers() {
    # $1 is an extra header line, or empty. A location that sets add_header
    # drops headers inherited from the server block, so callers repeat these.
    printf '        add_header X-Content-Type-Options "nosniff" always;\n'
    printf '        add_header Referrer-Policy "strict-origin-when-cross-origin" always;\n'
    printf '        add_header X-Frame-Options "DENY" always;\n'
    if [ -n "$1" ]; then
        printf '        %s\n' "$1"
    fi
}

app_locations() {
    hsts=$1
    cat <<EOF
    location ~ ^/(api/|auth/|healthz\$) {
        proxy_pass ${upstream};
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_set_header X-Forwarded-Host \$host;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;
    }

    location /assets/ {
        try_files \$uri =404;
        add_header Cache-Control "public, max-age=31536000, immutable" always;
EOF
    security_headers "$hsts"
    cat <<EOF
    }

    location / {
        try_files \$uri /index.html;
        add_header Cache-Control "no-cache" always;
EOF
    security_headers "$hsts"
    printf '    }\n'
}

{
    if [ "$redirect" = "1" ]; then
        cat <<EOF
server {
    listen 8080;
    server_name _;
    server_tokens off;
    root /usr/share/nginx/html;

    location ^~ /.well-known/acme-challenge/ {
        proxy_pass ${acme};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header Connection "";
    }

    location / {
        return 301 https://\$host\$request_uri;
    }
}
EOF
    else
        cat <<EOF
server {
    listen 8080;
    server_name _;
    server_tokens off;
    client_max_body_size 32m;
    root /usr/share/nginx/html;

    gzip on;
    gzip_types text/css application/javascript application/json;

    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    add_header X-Frame-Options "DENY" always;

EOF
        app_locations ""
        printf '}\n'
    fi

    if [ "$tls" = "1" ]; then
        cat <<EOF

server {
    listen 8443 ssl;
    http2 on;
    server_name _;
    server_tokens off;
    client_max_body_size 32m;
    root /usr/share/nginx/html;

    ssl_certificate ${cert};
    ssl_certificate_key ${key};
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers off;

    gzip on;
    gzip_types text/css application/javascript application/json;

    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    add_header X-Frame-Options "DENY" always;
    add_header Strict-Transport-Security "max-age=31536000" always;

EOF
        app_locations 'add_header Strict-Transport-Security "max-age=31536000" always;'
        printf '}\n'
    fi
} >"$tmp"

mv "$tmp" /etc/nginx/conf.d/default.conf
