# make install    build lokal and install the CLI into $(BINDIR); if lokal is
#                 already set up, also switch the background service to it
# make uninstall  remove the CLI from $(BINDIR) (the background service and the
#                 certificate stay; use `lokal uninstall` for those)

BINDIR ?= $(HOME)/.local/bin
LOKAL  := $(BINDIR)/lokal

.PHONY: build install uninstall

build:
	cargo build --release --workspace

install: build
	@mkdir -p "$(BINDIR)"
	install -m 755 target/release/lokal "$(LOKAL)"
	@if "$(LOKAL)" status --json 2>/dev/null | grep -q '"setup_done": true'; then \
		"$(LOKAL)" upgrade; \
	else \
		echo "Installed $(LOKAL). Next: lokal setup"; \
	fi
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "note: $(BINDIR) is not on your PATH";; esac

uninstall:
	rm -f "$(LOKAL)"
	@echo "Removed $(LOKAL). The background service is still installed; run 'lokal uninstall' to remove it."
