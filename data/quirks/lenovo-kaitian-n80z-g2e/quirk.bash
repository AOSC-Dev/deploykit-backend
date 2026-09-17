#!/bin/bash

# Enable error trapping.
set -Eeuo pipefail

# Lenovo KaiTian N80z-G2e does not support booting via ID.
grub-install \
	--force-extra-removable

# Update GRUB.
update-grub
