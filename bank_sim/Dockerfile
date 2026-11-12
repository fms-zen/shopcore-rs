FROM python:3.13-slim-bookworm
WORKDIR /srv/bank
RUN pip install --no-cache-dir Flask==3.1.2 requests==2.32.5
COPY app.py ./
EXPOSE 5001
CMD ["python", "app.py"]
