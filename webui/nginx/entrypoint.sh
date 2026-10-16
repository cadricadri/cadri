#!/bin/sh
# Render nginx config, then stay in the foreground. In the cluster, reload when
# the Let's Encrypt files show up or change so a renewal does not need a rollout.
set -eu

/cadri-render-nginx.sh
nginx -t

if [ "${CADRI_HTTPS_REDIRECT:-}" = "1" ]; then
    nohup /bin/sh -c '
        cert=/etc/cadri/tls/tls.crt
        key=/etc/cadri/tls/tls.key
        sum_certs() {
            if [ -s "$cert" ] && [ -s "$key" ]; then
                sha256sum "$cert" "$key"
            else
                echo absent
            fi
        }
        prev=$(sum_certs)
        while true; do
            sleep 10
            now=$(sum_certs)
            if [ "$now" != "$prev" ]; then
                # Keep the previous sum when reload fails so the next pass tries again.
                if /cadri-render-nginx.sh && nginx -s reload; then
                    prev=$now
                fi
            fi
        done
    ' >/tmp/cert-watch.log 2>&1 &
fi

exec nginx -g "daemon off;"
