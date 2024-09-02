FROM debian:12

RUN mkdir /app
RUN mkdir /appconf
RUN mkdir /appdata
RUN mkdir /logdata

# This is here because the binary is copied as an artefact in github actions
COPY ./centinela /app/
RUN chmod +x /app/centinela

ENTRYPOINT /app/centinela /appconf/config.yaml /appdata/data.json

