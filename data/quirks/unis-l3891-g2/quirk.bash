#!/bin/bash

# Enable error trapping.
set -Eeuo pipefail

# UNIS L3891 G2 does not support booting via ID.
grub-install \
	--force-extra-removable

# Update GRUB.
update-grub
