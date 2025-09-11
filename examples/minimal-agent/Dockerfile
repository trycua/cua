FROM python:3.11-slim

WORKDIR /app
COPY example_agent.py /app

RUN useradd -m agent && chown -R agent:agent /app
USER agent

CMD ["python", "example_agent.py"]

