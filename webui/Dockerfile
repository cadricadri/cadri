FROM node:24-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginxinc/nginx-unprivileged:stable-alpine
ENV CADRI_ENGINE_UPSTREAM=http://engine:8080 \
    CADRI_TLS_LISTEN_PARAMS= \
    CADRI_TLS_DIRECTIVES= \
    CADRI_PROXY_SSL_DIRECTIVES= \
    NGINX_ENVSUBST_FILTER=^CADRI_
COPY nginx/default.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 8080
