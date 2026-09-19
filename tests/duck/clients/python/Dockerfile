FROM python:3.12-slim
RUN pip install --no-cache-dir hvac==2.3.0
WORKDIR /app
COPY duck.py .
CMD ["python", "-u", "duck.py"]
