#!/bin/bash

# Enable error trapping.
set -Eeuo pipefail

# Huawei Qingyun W510 (some models, such as the D1041 revision) is known
# to shutdown between 60 and 240 seconds post-boot if the bootloader ID is
# not set to "ubuntu". Strange.
grub-install \
	--force-extra-removable \
	--bootloader-id="ubuntu"

# Update GRUB.
update-grub
