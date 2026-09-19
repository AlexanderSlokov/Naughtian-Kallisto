FROM golang:1.24-bookworm
WORKDIR /app
COPY go.mod ./
RUN go mod download github.com/hashicorp/vault/api 2>/dev/null || true
COPY duck.go ./
RUN go mod tidy && go build -o duck .
CMD ["./duck"]
