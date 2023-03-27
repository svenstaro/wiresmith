FROM gcr.io/distroless/cc
COPY --chmod=755 wiresmith /app/
ENTRYPOINT ["/app/wiresmith"]
