FROM debian:12

RUN mkdir /app
RUN mkdir /appconf
RUN mkdir /appdata
RUN mkdir /logdata

COPY ./target/x86_64-unknown-linux-musl/release/centinela /app/

ENTRYPOINT /app/centinela /appconf/config.yaml /appdata/data.json

