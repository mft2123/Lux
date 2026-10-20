FROM python:3.12-slim

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    TRANSFER115_HOST=0.0.0.0 \
    TRANSFER115_PORT=8098

WORKDIR /app

# The 115 client library and its transitive dependencies. The index is a build
# argument because pypi.org itself is unreachable from the deployment LAN; the
# default is the Tsinghua mirror, and `--build-arg PIP_INDEX_URL=...` overrides it.
ARG PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple
COPY requirements.txt .
RUN pip install --no-cache-dir --index-url "$PIP_INDEX_URL" -r requirements.txt

COPY transfer115 ./transfer115
COPY main.py .

# The sidecar only reads the shared settings file (the 115 cookie); it does not
# write to the archive root, so the config mount can stay read only.
EXPOSE 8098

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
  CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8098/healthz', timeout=5).status == 200 else 1)"

CMD ["python", "-u", "main.py"]
