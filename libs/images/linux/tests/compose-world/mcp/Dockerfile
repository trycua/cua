FROM mirror.gcr.io/library/python:3.12-slim@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1
RUN pip install --no-cache-dir mcp==2.3.0
COPY server.py /srv/server.py
CMD ["python", "/srv/server.py"]
