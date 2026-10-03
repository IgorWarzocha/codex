Choose only constraints that affect the requested asset. These are prompt decisions, not tool parameters.

## Text and structured layouts

- Quote copy verbatim. Specify typography and placement, require no extra characters, and spell troublesome words letter-by-letter.
- For raster UI mockups, name the fidelity first. Use practical hierarchy and controls, not concept-art language.
- For infographics, scientific diagrams, slides, and charts, supply actual labels and data, layout flow, arrows, and accuracy constraints. Inspect labels and relationships, not just visual polish.
- Reserve negative space only where the surrounding page needs copy. Do not invent left/right placement.

## Asset-specific constraints

- Natural photos: request photorealism with real skin, fabric, and material texture. Avoid heavy retouching when a candid result is intended.
- Product shots: specify materials, silhouette, label clarity, and controlled reflections.
- Raster logo concepts: use a strong silhouette and balanced negative space. "Vector-like" styling still produces a bitmap, not an editable vector.
- Game icons: preserve a readable silhouette, padding, and the established style. Tileable textures need seamless edges, neutral lighting, and no dominant focal element.
- Story panels: specify concrete actions per panel. Continue a character from a previous anchor image without redesigning facial features, proportions, outfit, or palette.
- Historical scenes: constrain clothing, props, and environment to the supplied place and date.

## Edit invariants

| Change | Preserve or match |
| --- | --- |
| Text localization | Layout, typography, spacing, hierarchy, logos, and imagery. Allow reflow only where necessary. |
| Clothing or person-in-scene | Face, body shape, pose, hair, expression, identity. Match lighting and shadows. |
| Object replacement | Camera angle, surrounding texture, lighting, and shadows. |
| Weather or time of day | Geometry, framing, subject identity. Change only environmental conditions. |
| Cutout | Fine edges and label text. Inspect alpha, halos, and restyling. |
| Style transfer | Requested palette, texture, and brushwork. Add no extra elements. |
| Compositing | Identify what moves from which input into which base. Match lighting, perspective, and scale. Preserve base framing. |
| Sketch to render | Layout, proportions, perspective. Add no unrequested elements. |
