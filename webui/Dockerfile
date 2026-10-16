FROM node:24-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginxinc/nginx-unprivileged:stable-alpine
USER root
COPY nginx/render-nginx.sh nginx/entrypoint.sh /
RUN chmod 755 /render-nginx.sh /entrypoint.sh \
    && mv /render-nginx.sh /cadri-render-nginx.sh \
    && mv /entrypoint.sh /cadri-entrypoint.sh
USER nginx
ENV CADRI_ENGINE_UPSTREAM=http://engine:8080 \
    CADRI_HTTPS_REDIRECT= \
    CADRI_ACME_UPSTREAM=
COPY --from=build /app/dist /usr/share/nginx/html
EXPOSE 8080 8443
ENTRYPOINT ["/cadri-entrypoint.sh"]
