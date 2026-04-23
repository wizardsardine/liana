# Avatar Prompt Guide — Kage Sumi-e Style

Server-side reference for assembling the image-generation prompt from
`AvatarUserTraits` + `AvatarDerivedTraits`.

Every generated avatar must look like it was painted by the same artist
as the Kage artwork in `design/artwork/sumi-e/kage/`. If placed next to
`kage-bust.png`, the two should be indistinguishable in medium.

---

## 1. Base Style Block

Prepend this to every assembled prompt, before any trait language.

```
Sumi-e ink wash painting of a samurai warrior in bust/portrait framing.
Match the exact art style of the attached Kage reference images.

INK TECHNIQUE — the most critical directives:
- Painted with real sumi ink on paper. Wet, saturated black ink with
  hard brush edges where the brush loads heavy, and dry streaked marks
  (kasure) where the brush runs low.
- HIGH CONTRAST: deep pure blacks against clean white/transparent space.
  No muddy mid-tones. The darks must be rich and saturated, not gray
  or chalky. The lights must be clean, not hazy.
- Gray washes are TRANSLUCENT and LAYERED — like diluted ink on wet
  paper, not opaque paint. You should sense the paper beneath the wash.
- Ink drips bleed downward from the figure. Splatters are BLACK INK,
  not white or chalk-colored.
- The figure dissolves into ink drips at the bottom edge of the frame.

COLOR — three tones only, no exceptions:
- Pure black sumi ink
- Translucent gray ink washes
- Bitcoin orange #F7931A (ensō, eyes, and one small accent only)
- Nothing else. No brown, no blue, no white paint, no skin tones,
  no metallic sheen, no chalk textures.

SIGNATURE ELEMENTS:
- Large BOLD orange ensō behind the head — thick brushstroke, slightly
  imperfect, with visible brush start/end points. The ensō must be
  PROMINENT and HEAVY, not thin or recessed.
- Glowing orange eyes — the ONLY visible facial feature
- Red hanko seal stamp, bottom-right corner
- Bust composition: head, shoulders, upper chest, centered

BACKGROUND:
- Fully transparent PNG (no solid background)
- No paper texture, no scenery

THIS IS NOT:
- Not 3D, not CGI, not photorealistic, not a texture map
- Not anime, not manga, not cartoon
- Not soft, not blended, not airbrushed — the brush edges must be SHARP
- Not chalky, not dusty, not matte clay — the ink must look WET
- Not low contrast — if the darks and lights don't punch, it is wrong
```

### Why specific phrases matter

| Problem in current avatars | Cause | Fix phrase |
|---------------------------|-------|------------|
| Muddy, low-contrast look | Model treats ink like a 3D texture | "HIGH CONTRAST: deep pure blacks against clean white space" |
| Clay/plaster surface feel | Model renders ink as a material on a surface | "Painted with real sumi ink on paper — wet, saturated, hard brush edges" |
| White/chalk splatters | Model adds highlight splatters | "Splatters are BLACK INK, not white or chalk-colored" |
| Thin, recessed ensō | Ensō treated as background element | "The ensō must be PROMINENT and HEAVY, not thin or recessed" |
| Gray muddy washes | Opaque gray paint instead of diluted ink | "Gray washes are TRANSLUCENT and LAYERED — like diluted ink on wet paper" |
| Soft blended edges | Airbrush/diffusion artifact | "Brush edges must be SHARP — not soft, not blended, not airbrushed" |

---

## 2. Trait → Prompt Mapping

### User Traits

#### `archetype`

| Value | Prompt |
|-------|--------|
| Ronin | "A wandering ronin — masterless, lean, travel-worn. Simple robes. Loose ink, more dry brush." |
| Samurai | "A disciplined samurai — upright, squared shoulders, clean armor. Controlled, deliberate strokes." |
| Shogun | "A commanding shogun — broad-shouldered, ornate headgear, heaviest armor. Dense, massive ink." |

#### `gender`

| Value | Prompt |
|-------|--------|
| Man | "Male figure. Broader shoulders, angular jaw shadow." |
| Woman | "Female onna-bugeisha warrior. Femininity from silhouette (narrower taper, elegant collar, kanzashi pins) — NOT from softened brushwork. Identical ink weight and intensity as male figures." |

#### `age_feel`

| Value | Prompt |
|-------|--------|
| Young | "Youthful — sharper angles, thinner build, more splash energy." |
| Mature | "Seasoned — balanced, steady, confident unhurried strokes." |
| Elder | "Weathered veteran — heavier ink pooling, more dry brush texture, trailing elements." |

#### `demeanor`

| Value | Prompt |
|-------|--------|
| Calm | "Composed and still. Restrained splatters, slow vertical drips." |
| Fierce | "Aggressive. Slashed brushstrokes, more splatter, ensō painted in one bold arc. Eyes burn brighter." |
| Mysterious | "Enigmatic. More shadow than form, heavy gray washes dissolving edges. Face deeper in shadow." |

#### `armor_style`

| Value | Prompt |
|-------|--------|
| Light | "Minimal armor — travel robe, light haori. Fluid ink washes for fabric folds." |
| Standard | "Traditional layered samurai armor — sode, chest plate. Heavy black ink, suggest plates without over-detailing." |
| Heavy | "Full heavy armor — massive pauldrons, thick chest plate. Densest ink in the image." |

#### `accent_motif`

| Value | Prompt |
|-------|--------|
| OrangeSun | "Bold full ensō as radiant sun disc — slightly larger, with dry brush rays at the edges." |
| Splatter | "Orange ink splatters scattered around shoulders and ensō — flicked from an orange brush." |
| Seal | "Prominent orange hanko seal stamp on the chest plate — larger than the corner seal." |
| Calligraphy | "Single bold orange calligraphic stroke across chest or shoulder." |

#### `laser_eyes`

| Value | Prompt |
|-------|--------|
| true | "Eyes emit visible orange light beams — twin horizontal rays cutting through shadow. Painterly glow, not digital lens flare." |
| false | (standard glowing eyes) |

### Derived Traits

#### `hat_style` (silhouette — most important differentiator)

| Value | Prompt |
|-------|--------|
| kabuto_horned | "Low wide kabuto with two short curved oni horns at the temples." |
| kabuto_crescent | "Tall narrow kabuto with a vertical crescent moon crest." |
| jingasa | "Wide-brimmed jingasa — broad flat disc extending past the shoulders." |
| eboshi | "Tall cylindrical eboshi court cap — stovepipe shape." |
| sugegasa | "Pointed sugegasa conical straw hat with weave texture." |
| hood_peaked | "Deep peaked fabric hood casting face in shadow. No metal." |
| cowl_rounded | "Heavy rounded sōhei monk's cowl, draped close to skull." |
| tengai_basket | "Full tengai basket covering entire head. Eyes glow faintly through weave." |
| crown | "Ornate pointed crown in dark ink with orange gemstone accents." |
| headwrap | "Tenugui headwrap in Edo craftsman style, flat-topped, trailing tails." |
| kanzashi_veil | "Kanzashi hairpins fanning upward from topknot with trailing white silk veil." |

#### `pose`

| Value | Prompt |
|-------|--------|
| frontal | "Facing forward, symmetrical." |
| three_quarter | "Three-quarter turn, one shoulder closer." |
| profile | "Near-profile, face turned ~70° to one side." |

#### `enso_style`

| Value | Prompt |
|-------|--------|
| full | "Complete ensō — bold, imperfect, visible brush lift point." |
| broken | "Broken ensō — deliberate gap in the circle." |
| double | "Double ensō — inner bright orange, outer faded gray." |
| splashed | "Splashed ensō — a burst of orange suggesting the circle's energy." |

#### `ink_density`

| Value | Prompt |
|-------|--------|
| light | "Light ink — more dry brush and white space. Figure is suggested, not fully rendered." |
| medium | "Balanced — confident black masses, gray washes, breathing room." |
| heavy | "Heavy ink — dense saturated black, minimal white space. Orange burns through dark mass." |

#### `brush_texture`

| Value | Prompt |
|-------|--------|
| smooth | "Long flowing strokes, minimal bristle marks." |
| dry | "Dominant dry brush — streaked, scratchy, paper showing through." |
| wet | "Wet saturated ink — bleeds and feathers at edges, pools where brush paused." |

#### `weapon_mode`

| Value | Prompt |
|-------|--------|
| katana | "Katana hilt rising behind one shoulder, handle wrapped in orange cord." |
| wakizashi | "Two short wakizashi crossed behind shoulders." |
| naginata | "Naginata polearm haft rising behind one shoulder." |
| none | "No visible weapons." |
| flute | "Shakuhachi bamboo flute held across chest." |

#### Other derived traits

Map `face_visibility`, `eye_visibility`, `shoulder_profile`, `cloak_presence`,
`armor_wear`, `splash_intensity`, `orange_placement`, and `ornament_level`
to short descriptive phrases. Keep each to one sentence. Less is more —
the base style block does the heavy lifting.

---

## 3. Prompt Assembly Order

```
1. Base Style Block (Section 1)
2. Archetype
3. Gender
4. Age feel
5. Demeanor
6. Hat / silhouette
7. Pose
8. Face and eye treatment
9. Armor
10. Weapon
11. Ensō style
12. Ink density + brush texture
13. Accent motif
14. Laser eyes (if on)
15. Negative prompt
```

### Negative prompt (always append)

```
3D render, CGI, photorealistic, anime, manga, cartoon, neon, colorful,
gradient, watermark, text overlay, metallic sheen, lens flare, glossy,
smooth digital shading, vector art, clean lines, airbrushed, soft focus,
chalk texture, clay texture, matte surface, low contrast, muddy tones,
white splatters, skin tones, any color besides black/gray/orange
```

---

## 4. Style Drift Recovery

| Problem | Inject this |
|---------|-------------|
| Too 3D / textured | "This is flat ink on paper, not a 3D texture map. The surface is PAPER, not clay or stone." |
| Low contrast / muddy | "Increase contrast. Pure saturated blacks against clean white space. No muddy mid-tones." |
| Soft / blended edges | "Sharpen all brush edges. Sumi-e brushstrokes have HARD edges where ink meets paper. No airbrushing." |
| Ensō too thin | "The ensō must be painted with a THICK, fully loaded brush. Bold and heavy, not delicate." |
| White splatters | "All splatters must be BLACK INK. Remove any white, chalk, or plaster-colored marks." |
| Looks like a painting OF a statue | "This is a painting, not a painting of a 3D object. No sculptural lighting, no surface texture." |
| Orange bleeding everywhere | "Orange appears ONLY in the ensō, eyes, and one accent. Everything else is black ink and gray wash." |
| Too anime | "Traditional sumi-e, not anime. Think Sesshū Tōyō, not Masashi Kishimoto." |
| Female figure softened | "Identical brushwork weight and splatter intensity as male figures. Silhouette alone indicates gender." |

---

## 5. Reference Image Anchoring

If the image generation API supports style/reference images, upload 2–3
of these alongside every prompt:

- `kage-bust.png` — the gold standard for bust framing and ink technique
- `kage-front.png` — standing pose, full armor detail
- `kage-lightning-send.png` — dynamic pose with ensō

This is the single highest-leverage improvement. Without reference images,
models drift toward generic digital samurai art within 1–2 generations.
