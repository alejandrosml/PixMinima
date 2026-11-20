# PixMinima, ImageOptim et Chisel sur M1 Max

[English](comparison.en.md) · [Utiliser PixMinima](../README.fr.md)

Sur le lot principal de onze images, PixMinima Maximum produit **1 693 165 octets**, soit **2 355 octets de moins que Chisel à effort élevé**. Il gagne sur 8 fichiers, et Chisel sur 3. Le traitement du lot prend respectivement 95,42 et 57,40 secondes. Le gain publié est le rapport entre les octets économisés et le poids total de départ.

## Onze photos et illustrations

Huit PNG et trois JPEG publics de scikit-image totalisent 2 170 323 octets. Les trois passages de chaque configuration utilisent des copies fraîches sur le même M1 Max. La dernière colonne compte les résultats pour lesquels le protocole a retenu la taille du fichier source après vérification. Un fichier rendu identique par l’outil compte normalement dans la taille totale.

| Logiciel et réglage | Taille retenue (octets) | Gain total | Temps médian | Originaux retenus après le contrôle |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 1 693 165 | 21,99 % | 95,42 s | 0 |
| Chisel, effort élevé | 1 695 520 | 21,88 % | 57,40 s | 0 |
| PixMinima, approfondi | 1 700 035 | 21,67 % | 23,54 s | 0 |
| Chisel, standard | 1 707 276 | 21,34 % | 6,04 s | 0 |
| ImageOptim, moteurs réglés pour conserver les données | 1 715 047 | 20,98 % | 56,23 s | 0 |
| PixMinima, rapide | 1 809 018 | 16,65 % | 0,15 s | 0 |
| ImageOptim, réglages d’origine | 2 154 212 | 0,74 % | 20,04 s | 8 |

ImageOptim 1.9.3, signé par son éditeur, a réellement produit 1 674 780 octets avec ses réglages d’origine. Les onze images sont visuellement préservées sur ce lot. Sur huit d’entre elles, certaines métadonnées ont changé. Le tableau utilise donc la taille des huit originaux correspondants. ImageOptim permet également de garder les métadonnées. La ligne « moteurs réglés » mesure une sélection explicite de ses exécutables embarqués. Les réglages personnels de l’application ouverte n’ont pas servi aux essais. [Fonctionnement d’ImageOptim](https://imageoptim.com/mac) · [Commande officielle de l’application graphique](https://imageoptim.com/command-line.html)

Sur les huit PNG, PixMinima Maximum totalise 838 594 octets et Chisel à effort élevé 840 999. Sur les trois JPEG, les totaux sont 854 571 et 854 521 octets. La légère avance sur l’ensemble dépend donc des fichiers.

## Six autres images publiques

Ce corpus distinct inclut notamment un TIFF multipage 16 bits. Il pèse 251 696 octets au départ.

| Logiciel et réglage | Taille retenue (octets) | Gain total | Temps médian | Originaux retenus après le contrôle |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 146 143 | 41,94 % | 30,00 s | 0 |
| Chisel, effort élevé | 147 936 | 41,22 % | 7,11 s | 0 |
| PixMinima, approfondi | 149 442 | 40,63 % | 12,49 s | 0 |
| Chisel, standard | 150 842 | 40,07 % | 1,09 s | 0 |
| ImageOptim, moteurs réglés pour conserver les données | 229 508 | 8,82 % | 5,78 s | 2 |
| PixMinima, rapide | 159 967 | 36,44 % | 0,18 s | 0 |
| ImageOptim, réglages d’origine | 247 601 | 1,63 % | 5,22 s | 2 |

## Vingt cas de conservation

Ce lot construit de 35 000 octets couvre notamment les animations, la transparence, les couleurs invisibles sous alpha, les profils, les textes et des conteneurs. Il sert à repérer les différences de comportement entre formats. Les proportions de formats de ce lot technique diffèrent de celles d’une photothèque.

| Logiciel et réglage | Taille retenue (octets) | Gain total | Temps médian | Originaux retenus après le contrôle |
| --- | ---: | ---: | ---: | ---: |
| PixMinima, Maximum | 12 383 | 64,62 % | 0,37 s | 0 |
| Chisel, effort élevé | 18 878 | 46,06 % | 0,65 s | 3 |
| PixMinima, approfondi | 12 399 | 64,57 % | 0,14 s | 0 |
| Chisel, standard | 18 881 | 46,05 % | 0,07 s | 3 |
| ImageOptim, moteurs réglés pour conserver les données | 25 413 | 27,39 % | 0,86 s | 15 |
| PixMinima, rapide | 12 520 | 64,23 % | 0,11 s | 0 |
| ImageOptim, réglages d’origine | 35 000 | 0,00 % | 5,64 s | 9 |

Une optimisation interne de PNG dans une archive OpenRaster est créditée quand les couches décodées, leurs métadonnées et les propriétés de l’archive sont conservées. Le test GIF compare aussi le nombre d’images et leurs durées. Le test PPM conserve les commentaires. Pour un JPEG suivi de données opaques, le protocole exige l’identité du fichier entier afin de conserver les positions de ces données. Ce dernier contrôle peut écarter un fichier dont tous les pixels et les octets de la remorque ont été préservés.

## CPU et Metal sur le même Mac

Sur une image RGBA 3 840 × 2 160, le choix du filtre par ligne a pris 11,03 ms sur le CPU et 2,60 ms sur le GPU une fois celui-ci initialisé. Le calcul complet comprend aussi la lecture, la compression DEFLATE, la vérification et le remplacement. Sur neuf PNG 4K différents, les médianes du traitement complet sont de 13,72 s en CPU, 13,52 s avec Metal forcé et 13,24 s en automatique. Sur treize images de tailles variées, elles sont respectivement de 13,62 s, 15,01 s et 14,13 s. Dans les 18 lots le contenu de chaque fichier de sortie était identique à l’octet près, quel que soit le mode. Trois passages par configuration laissent une marge de variation, particulièrement visible dans les essais proches. [Mesures et empreintes par fichier](../bench/metal/)

## Choisir un outil

- **PixMinima** associe le remplacement sans sauvegarde, le suivi du gain par image, les réglages de langue et le choix entre plusieurs efforts de recherche. Le moteur garde les métadonnées prises en charge et propose des optimisations spécifiques aux composites Photoshop, aux ressources d’icônes et aux documents à calques. Pour les formats HEIC, HEIF, AVIF et JPEG XL, l’optimisation standard porte sur le conteneur. [Formats et limites](formats.md)
- **ImageOptim** offre une interface Mac par glisser-déposer et prend en charge des versions de macOS antérieures à Golden Gate. Son application Cocoa comporte du code Objective-C et pilote plusieurs optimiseurs. Le code de l’application est sous GPL version 2 ou ultérieure. Le composant PNGOUT embarqué possède une licence distincte. [Application et licence](https://imageoptim.com/mac) · [Code source](https://github.com/ImageOptim/ImageOptim)
- **Chisel** est un outil en ligne de commande principalement écrit en C++, publié sous licence MIT. Il couvre de nombreuses familles de fichiers au-delà de l’image. Son réglage élevé a traité les deux corpus publics plus vite que PixMinima Maximum sur ce Mac. [Code et licence](https://github.com/Snesnopic/chisel)
- **Autres projets libres.** Oxipng se spécialise dans les PNG et APNG et propose une bibliothèque Rust. Efficient Compression Tool est écrit en C++ et traite notamment PNG, JPEG, ZIP et GZIP. Sur les huit PNG du lot principal, ECT produit 835 928 octets avant le contrôle des métadonnées et 871 418 octets selon les critères du comparatif. Oxipng produit respectivement 839 909 et 1 216 615 octets. PixMinima Maximum atteint 838 594 octets après contrôle. [Mesures et raisons des écarts](../bench/specialists/) [Oxipng](https://github.com/oxipng/oxipng) · [Efficient Compression Tool](https://github.com/fhanau/Efficient-Compression-Tool)

Les comparaisons de taille ci-dessus utilisent le mode sans perte de PixMinima. Les essais de son option avec perte ont leur propre protocole. [Qualité PNG](../bench/lossy/PNG.md) · [Qualité JPEG, GIF et WebP](../bench/lossy/formats.md)

Toutes les mesures ont été faites le 20 novembre 2026 sur un M1 Max sous macOS 27.2.1. Le lot principal a aussi servi aux vérifications pendant le développement. Les sorties, les empreintes, les médianes, les temps individuels et les conditions de vérification sont publiés avec le protocole. [Données du comparatif](../bench/) · [Mesures CPU et Metal](../bench/metal/)
