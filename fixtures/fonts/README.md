# Bundled generic faces

Engine defaults are full upstream files, without subsetting or modification.
No font discovery, embedding flag checks, or license checks run in the engine.

| Generic | File | License | Bytes | SHA-256 | Upstream revision |
| --- | --- | --- | ---: | --- | --- |
| serif | SourceSerifPro-Regular.otf | SIL OFL 1.1, SourceSerif-LICENSE.md | 217280 | 199c8adc83479480d9aa5d942afe8f52085af4e6c2093819b190807702fbcfe2 | existing fixture |
| sans-serif | SourceSans3-Regular.otf | SIL OFL 1.1, SourceSans3-LICENSE.md | 334924 | 08df266400933d3178d081a45f94a08814c3e55b4b7dd2e0ff69cb1329f13ab6 | adobe-fonts/source-sans@87b37a2daaed80fcb8e8ccb0085c4d72ddade12e |
| monospace | SourceCodePro-Regular.otf | SIL OFL 1.1, SourceCodePro-LICENSE.md | 131128 | 9f9664e2edf6f045c11e774f9bd0be6993971f2544a39061a5ce478b96b051f8 | adobe-fonts/source-code-pro@803b7e23ec97ae58b6232ea76519a76d428ba268 |
| script | DancingScript-wght.ttf | SIL OFL 1.1, DancingScript-OFL.txt | 133636 | 21808625578fe8d8cd10cb684be546dca077b27cd03a53a2f1ec11dc743c924c | google/fonts@9710da1eacb3be272583c3224dcb70f9da6eadbb |

Dancing Script uses its default variation location (Regular). Frontend descriptor
matching chooses registered faces; variable-axis instancing is a future extension.
