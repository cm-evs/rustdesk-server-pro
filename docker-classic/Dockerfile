FROM scratch
COPY hbbs /usr/bin/hbbs
COPY hbbr /usr/bin/hbbr
EXPOSE 21114 21115 21116 21116/udp 21117 21118 21119
WORKDIR /root
ENV HOME=/root
# Keep the working directory (/root, the volume mount point) instead of
# anchoring to the binary directory (/usr/bin), so runtime data
# (databases, keys, logs) lands in the mounted volume.
ENV RD_SKIP_ANCHOR=1
