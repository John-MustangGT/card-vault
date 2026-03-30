PREFIX = /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
DATADIR = $(PREFIX)/share/mtg-labeler
COMPDIR = /etc/bash_completion.d

install:
	install -d $(BINDIR) $(MANDIR) $(DATADIR)
	install -m 755 labeler.py $(BINDIR)/mtg-labeler
	install -m 644 mtg-labeler.1 $(MANDIR)/mtg-labeler.1
	install -m 644 header.ps $(DATADIR)/header.ps
	@if [ -d $(COMPDIR) ]; then \
		install -m 644 mtg-labeler.bash $(COMPDIR)/mtg-labeler; \
	fi

clean:
	rm -f *.ps *.pdf
