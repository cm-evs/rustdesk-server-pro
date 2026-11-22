FROM busybox:stable

ARG S6_OVERLAY_VERSION=3.2.0.0
ARG S6_ARCH=x86_64
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-noarch.tar.xz /tmp
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-${S6_ARCH}.tar.xz /tmp
RUN \
  tar -C / -Jxpf /tmp/s6-overlay-noarch.tar.xz && \
  tar -C / -Jxpf /tmp/s6-overlay-${S6_ARCH}.tar.xz && \
  rm /tmp/s6-overlay*.tar.xz && \
  ln -s /run /var/run

COPY rootfs /

ENV RELAY=relay.example.com
ENV ENCRYPTED_ONLY=0
# Keep the working directory (/data) instead of anchoring to the binary
# directory (/usr/bin), so runtime data (databases, keys, logs) lands in
# the volume and hbbs reads the key pair the key-secret service wrote
# to /data. Without this, the s6 run scripts' `cd /data` is undone at
# startup and every rebuild loses the data stored in /usr/bin.
ENV RD_SKIP_ANCHOR=1

EXPOSE 21114 21115 21116 21116/udp 21117 21118 21119

HEALTHCHECK --interval=10s --timeout=5s CMD /usr/bin/healthcheck.sh

WORKDIR /data

VOLUME /data

ENTRYPOINT ["/init"]
