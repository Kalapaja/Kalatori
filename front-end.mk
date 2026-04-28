# Front-end release version compatible with the current daemon version.
#
# This file is included from the top-level Makefile. CI hashes it as a cache key
# for the `static/` directory, so bumping the front-end version busts only that
# cache and leaves the tool cache untouched.

front_end_version := 0.0.25
