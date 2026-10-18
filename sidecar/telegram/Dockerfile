FROM python:3.12-slim

# 注意：不设 ENV UMASK —— 镜像构建参数不会作用到进程 umask，
# 真正的 umask(0o077) 在 main.py / tgwatch.runner 里设置。
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1 \
    TZ=Asia/Shanghai

WORKDIR /app

COPY requirements.txt ./
RUN pip install --no-cache-dir -r requirements.txt

COPY main.py ./
COPY tgwatch ./tgwatch

# 非 root 运行：session 里是账号授权密钥，不该由 root 独占（运维要能轮换/删除）
RUN groupadd -g 10001 lux \
    && useradd -u 10001 -g 10001 -M -s /usr/sbin/nologin lux \
    && mkdir -p /app/session /app/state /watch-inbox \
    && chown -R 10001:10001 /app/session /app/state /watch-inbox \
    && chmod 700 /app/session /app/state \
    && chmod 755 /watch-inbox
USER 10001:10001

VOLUME ["/app/session", "/app/state"]

# 没 session 时 check 返回非零 → 容器明确 unhealthy，而不是假装活着
HEALTHCHECK --interval=60s --timeout=10s --start-period=30s --retries=3 \
    CMD python main.py check > /dev/null || exit 1

ENTRYPOINT ["python", "main.py"]
CMD ["run"]
