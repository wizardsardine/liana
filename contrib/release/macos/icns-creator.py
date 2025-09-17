# To be run from the project root

from PIL import Image
from icnsutil import IcnsFile

# Input PNG (should be at least 1024x1024)
input_file = "liana-ui/static/logos/liana-app-icon-coincube.png"
output_file = "contrib/release/macos/Vault.icns"
output_ico = "contrib/release/wix/Vault.ico"

print(f"Processing {input_file}")

# Sizes macOS expects
sizes = [16, 32, 128, 256, 512]

img = Image.open(input_file).convert("RGBA")
icns = IcnsFile()

for size in sizes:
	# Standard size
	filename = f"contrib/icons/{size}x{size}.png"
	resized = img.resize((size, size), Image.LANCZOS)
	resized.save(filename)

	print(f"Created {filename}")

	# Retina (@2x) size
	filename2x = f"contrib/icons/{size}x{size}@2x.png"
	resized2x = img.resize((size*2, size*2), Image.LANCZOS)
	resized2x.save(filename2x)

	print(f"Created {filename2x}")

	icns.add_media(file=filename)
	icns.add_media(file=filename2x)

# Write to filesystem
icns.write(output_file)

print(f"Created {output_file}")

# Prepare Windows ICO
img.save(output_ico, format="ICO", sizes=[(16,16), (32,32), (48,48), (64,64)])
print(f"Created {output_ico}")
