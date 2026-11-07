"""Entry point for `python -m handset`."""

import sys


def main() -> None:
    if (
        "--server-child" in sys.argv
        or "--server-only" in sys.argv
        or ("--port" in sys.argv and "--gui" not in sys.argv and "--smoke-test" not in sys.argv)
    ):
        from handset.server import main as server_main
        server_main()
    else:
        from handset.gui import main as gui_main
        gui_main()


if __name__ == "__main__":
    main()
