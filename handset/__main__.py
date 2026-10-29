"""Entry point for `python -m handset`."""

import sys


def main() -> None:
    if "--server-only" in sys.argv:
        from handset.server import main as server_main
        server_main()
    else:
        from handset.gui import main as gui_main
        gui_main()


if __name__ == "__main__":
    main()

