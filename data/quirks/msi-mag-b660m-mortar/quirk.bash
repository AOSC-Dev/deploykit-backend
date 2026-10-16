#!/bin/bash

# Enable error trapping.
set -Eeuo pipefail

# Asus XC-KX700M D4 does not support booting via ID.
grub-install \
	--force-extra-removable

# Update GRUB.
update-grub
