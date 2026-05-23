# Local Makefile for Kallisto Documentation
# Run these commands directly inside the docs/ directory

.PHONY: all serve build clean help

all: build

help:
	@echo "Kallisto Docs Commands:"
	@echo ""
	@echo "  make serve    - Start local Hugo development server with draft support"
	@echo "  make build    - Compile Hugo documentation into static HTML"
	@echo "  make clean    - Remove generated public/ directory and resources cache"
	@echo ""

serve:
	hugo server -D

build:
	hugo

clean:
	rm -rf public resources
	@echo "Cleaned generated public files and resources."
