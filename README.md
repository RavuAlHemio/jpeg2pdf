# jpeg2pdf

Creates a single-page PDF document from a JPEG image.

The only modifications made to the JPEG image are the optional removal of unimportant metadata
blocks; the pixel data is transferred into the PDF completely unchanged.

The overhead of the PDF file is almost constant; above a specific JPEG file size, the file size of
the resulting PDF file is comparable.
