GUEST_BIN ?= ../target/x86_64-unknown-linux-musl/release/hollow-guest
OUT_DIR ?= out

.PHONY: base terraform-v1 fluxv1 docker-test all clean

all: terraform-v1 fluxv1

$(OUT_DIR):
	mkdir -p $(OUT_DIR)

# Build guest binary (requires musl target installed)
$(GUEST_BIN):
	cargo build -p hollow-guest --release --target x86_64-unknown-linux-musl

# Base image — all others derive from this
base: $(GUEST_BIN)
	cp $(GUEST_BIN) hollow-guest
	docker build -t hollow-base -f Dockerfile.base .
	rm -f hollow-guest

terraform-v1: base $(OUT_DIR)
	docker build -t hollow-terraform-v1 -f Dockerfile.terraform-v1 .
	./scripts/pack-ext4.sh hollow-terraform-v1 $(OUT_DIR)/terraform-v1.ext4 2048

fluxv1: base $(OUT_DIR)
	docker build -t hollow-fluxv1 -f Dockerfile.fluxv1 .
	./scripts/pack-ext4.sh hollow-fluxv1 $(OUT_DIR)/fluxv1.ext4 2048

docker-test: base $(OUT_DIR)
	docker build -t hollow-docker-test -f Dockerfile.docker-test .
	./scripts/pack-ext4.sh hollow-docker-test $(OUT_DIR)/docker-test.ext4 4096

clean:
	rm -rf $(OUT_DIR) hollow-guest
