# Numbered images and generated shot requirements

This is an acceptance design for extending the local media workflow. It does not add implemented commands or capability points. Current PNG scene support remains bounded by [SCENES.md](SCENES.md); the production coordinator remains planned under the [pipeline acceptance plan](pipeline/ACCEPTANCE.md).

## Footage identity and timing

A numbered sequence needs a declared first number, complete ordered file list, exact rational frame rate and immutable identity for each source. Repeated image content still occupies distinct time intervals. Validate every expected file before accepting a sequence; a valid first image does not establish a complete clip. Missing or corrupt middle frames require an explicit failure or a separately approved loss policy. Relinking must preserve declared timeline intervals and verify the replacement sequence's count and rate.

Use the complete source identity when reusing imported footage or cached frames. A filename or matching first image is insufficient. Verify a replacement's rendered content as well as its reported path; a successful import or relink response does not establish correct decoding of every frame.

Distinguish source animation holds from the constant-rate files produced by sampling them. Record the sampling convention, loop count, one-shot ending and any frame stepping. Preserve the chosen cadence when conforming to another timeline rate; do not silently add interpolation. Test first/last samples, very short poses, repeated cycles and loops whose duration is not an integer number of output frames. Store exact rates and derive duration from frame count, rather than trusting a rounded seconds field.

Render-range acceptance must compare the physical output count and final frame with the declared half-open interval. A configured duration alone is insufficient; an unintended extra or blank endpoint frame is a failed export.

## Pixel layout and transparency

Declare contain/cover fitting, integer enlargement, anchor coordinates and clipping independently. Cover fitting can crop part of a source pixel at the destination edge. Pixel-preserving output needs an independently verified nearest-neighbor route or externally baked geometry with its editable recipe retained. Baked pixels do not constitute editable timeline transforms.

Declare straight or premultiplied alpha and the source/working/output color conventions. Check translucent edges over multiple backgrounds and through the selected intermediate format. An encoder's success message, codec name or color tags alone do not establish preservation. Opaque SDR delivery requires a tested conversion and measured chroma-edge behavior; it must not be described as a lossless pixel-art route.

Verify each selected preset, resolution and output precision. An image format that supports transparency may still be exported without it. Record matrix, transfer, range and alpha association alongside the file, compare these declarations with the actual metadata, and reject ambiguity unless the workflow supplies an explicit tested interpretation. Do not silently infer a color matrix from frame dimensions or substitute a different decoder interpretation to declare a failed default route successful. Preserve the failed comparison and document the selected route separately.

Rasterized lettering is image content. Keep any editable text recipe and timed caption track separate, and report loss of those semantics during interchange. Refuse a request that claims editable text or captions from a raster-only source unless an explicit supported reconstruction operation has been performed.

## Coverage mapping

These requirements refine existing acceptance areas: M01/M03 for ingestion and relinking; T05/V07 for timing; V01/V02 for layout/compositing; C01/E02/E06 for declared color, delivery and output validation; G01-G03/I02 for graphics and semantic loss. They do not create additional engine acceptance points, change the existing denominator, or close the broader criteria by themselves.

Use original fixtures and independent frame, geometry, alpha and color references. Keep input hashes, exact tool versions, selected settings, output counts, failed attempts and supported-route limits with the external evidence.
