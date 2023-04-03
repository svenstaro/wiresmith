FROM docker.io/alpine
COPY --chmod=755 wiresmith /app/
RUN apk add wireguard-tools
ENTRYPOINT ["/app/wiresmith"]
