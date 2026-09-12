# Protocole de contrôle Conduit

<!-- Généré par `cargo run -p conduit-protocol --features schema --example gen-docs` ; ne pas éditer. -->

Version du protocole : **1**.

## Transport

Socket Unix (Linux, macOS) ou named pipe (Windows), jamais le réseau. Chaque message est une trame : `u32` petit-boutiste (longueur de la charge) puis charge **MessagePack** (champs nommés). Charge maximale : 1 MiB. Un client envoie d'abord `hello`, le démon répond `hello_reply`.

Les exemples ci-dessous sont en JSON pour la lisibilité ; l'encodage réel est MessagePack avec les mêmes clés.

## Commandes (`msg = request`, champ `command.cmd`)

- `status`
- `nodes`
- `ports`
- `links`
- `link`
- `link_by_name`
- `unlink`
- `set_node_gain`
- `set_link_gain`
- `set_label`
- `set_driver`
- `add_internal`
- `remove_node`
- `set_param`
- `read_meter`
- `cable_list`
- `cable_add`
- `cable_remove`
- `cable_rename`
- `cable_set_channels`
- `cable_set_format`
- `reset_xruns`
- `subscribe`
- `dump`
- `save`
- `load`

## Réponses (`msg = response`, `result.Ok.reply`)

- `ok`
- `status`
- `nodes`
- `node`
- `links`
- `link`
- `meter`
- `cables`
- `cable`
- `dump`

## Événements (`msg = event`, champ `notification.event`)

- `node_added`
- `node_removed`
- `node_state_changed`
- `link_added`
- `link_removed`
- `driver_changed`
- `cable_changed`
- `rt`
- `shutdown`

## Exemple

```json
{
  "msg": "request",
  "id": 1,
  "command": {
    "cmd": "status"
  }
}
```

## Schéma JSON complet

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Message",
  "description": "Tout message circulant sur le transport, dans les deux sens.",
  "oneOf": [
    {
      "description": "Client → démon, premier message.",
      "type": "object",
      "properties": {
        "msg": {
          "type": "string",
          "const": "hello"
        }
      },
      "$ref": "#/$defs/Hello",
      "required": [
        "msg"
      ]
    },
    {
      "description": "Démon → client, en réponse à `Hello`.",
      "type": "object",
      "properties": {
        "msg": {
          "type": "string",
          "const": "hello_reply"
        }
      },
      "$ref": "#/$defs/HelloReply",
      "required": [
        "msg"
      ]
    },
    {
      "description": "Client → démon.",
      "type": "object",
      "properties": {
        "msg": {
          "type": "string",
          "const": "request"
        }
      },
      "$ref": "#/$defs/Request",
      "required": [
        "msg"
      ]
    },
    {
      "description": "Démon → client.",
      "type": "object",
      "properties": {
        "msg": {
          "type": "string",
          "const": "response"
        }
      },
      "$ref": "#/$defs/Response",
      "required": [
        "msg"
      ]
    },
    {
      "description": "Démon → abonnés.",
      "type": "object",
      "properties": {
        "msg": {
          "type": "string",
          "const": "event"
        }
      },
      "$ref": "#/$defs/Event",
      "required": [
        "msg"
      ]
    }
  ],
  "$defs": {
    "CableFormat": {
      "description": "Le format d'un câble : fréquence, profondeur, canaux.\n\nC'est **le** type que la chaîne client transporte, du `conduitctl cable set-format` au\ncontrôle de la plateforme. Sous Windows il finit encodé dans le `REG_DWORD`\n`CableFormat<n>` du devnode ; ailleurs, il décrit simplement les endpoints que le\ndorsal publie.\n\nLe texte de [`FromStr`] est `fréquence:profondeur:canaux` — `48000:f32:2` —, le même\npour la ligne de commande et pour le TOML, parce qu'il n'y a aucune raison qu'un\nutilisateur apprenne deux orthographes du même réglage.",
      "type": "object",
      "properties": {
        "channels": {
          "description": "Nombre de canaux.",
          "$ref": "#/$defs/ChannelCount"
        },
        "depth": {
          "description": "Profondeur d'échantillon.",
          "$ref": "#/$defs/SampleDepth"
        },
        "sample_rate": {
          "description": "Fréquence d'échantillonnage.",
          "$ref": "#/$defs/SampleRate"
        }
      },
      "required": [
        "sample_rate",
        "depth",
        "channels"
      ]
    },
    "CableId": {
      "description": "Identifiant d'un câble (numéro stable, 1 = « Conduit 1 »).",
      "type": "integer",
      "format": "uint32",
      "minimum": 0
    },
    "CableInfo": {
      "description": "Description d'un câble existant.",
      "type": "object",
      "properties": {
        "active": {
          "description": "Vrai si actif (visible des applications).",
          "type": "boolean"
        },
        "capture": {
          "description": "Périphérique de capture (les applications y lisent).",
          "$ref": "#/$defs/DeviceId"
        },
        "channels": {
          "description": "Canaux — **raccourci sur `format.channels`**, jamais une seconde vérité.\n\nLe champ précède [`Self::format`] et lui survit le temps que ses appelants\nmigrent : la GUI, les règles d'auto-connexion et la table de `conduitctl cable\nlist` le lisent encore. Les implémentations du trait doivent le tenir **égal** à\n`format.channels` ; les tests l'assertent, et la dette « supprimer `channels` » est\nouverte.",
          "$ref": "#/$defs/ChannelCount"
        },
        "format": {
          "description": "Format servi par ce câble.\n\nNon optionnel : depuis M1b-05 chaque câble a le sien, et depuis le lot A1 la\nréponse `lister` du service porte la table des seize — le format est donc\n**toujours** connu. Un mot nul (clé matérielle illisible, câble hors réserve) se\nreplie sur [`CableFormat::default`], comme les canaux se repliaient auparavant,\nmais c'est désormais le cas exceptionnel et non l'ordinaire.",
          "$ref": "#/$defs/CableFormat"
        },
        "id": {
          "description": "Identifiant.",
          "$ref": "#/$defs/CableId"
        },
        "name": {
          "description": "Nom OS.",
          "type": "string"
        },
        "render": {
          "description": "Périphérique de rendu (les applications y jouent).",
          "$ref": "#/$defs/DeviceId"
        }
      },
      "required": [
        "id",
        "name",
        "channels",
        "format",
        "active",
        "render",
        "capture"
      ]
    },
    "CableSpec": {
      "description": "Demande de création ou de modification.",
      "type": "object",
      "properties": {
        "channels": {
          "description": "Canaux.\n\n**Ignoré quand [`Self::format`] est `Some`** : le format porte déjà ses canaux, et\nles faire dire par deux champs ferait une contradiction possible. C'est\n`format.channels` qui compte alors.",
          "$ref": "#/$defs/ChannelCount"
        },
        "format": {
          "description": "Format à appliquer **avant** l'activation, ou `None` pour prendre le câble tel\nqu'il est.\n\n`None` est le comportement d'avant M1b-05 : on connecte le câble sans toucher à sa\nclé matérielle. `Some` demande la seule séquence qui produise un endpoint au bon\nformat — écrire `CableFormat<n>`, redémarrer le devnode, **puis** activer — parce\nque le format d'un endpoint audio est figé à sa création et qu'un redémarrage du\npériphérique ne le déplace pas (mesuré en M1b-05).",
          "anyOf": [
            {
              "$ref": "#/$defs/CableFormat"
            },
            {
              "type": "null"
            }
          ],
          "default": null
        },
        "name": {
          "description": "Nom OS souhaité (`None` = `Conduit N`).",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "channels"
      ]
    },
    "ChannelCount": {
      "type": "integer",
      "maximum": 8,
      "minimum": 1
    },
    "ChannelLabel": {
      "description": "Position d'un canal dans une disposition standard.\n\nSert à l'adaptation automatique de canaux (F-15) et aux règles d'auto-connexion.",
      "oneOf": [
        {
          "description": "Mono ou sans position définie.",
          "type": "string",
          "const": "MONO"
        },
        {
          "description": "Avant gauche.",
          "type": "string",
          "const": "FL"
        },
        {
          "description": "Avant droit.",
          "type": "string",
          "const": "FR"
        },
        {
          "description": "Centre.",
          "type": "string",
          "const": "FC"
        },
        {
          "description": "Caisson de basses.",
          "type": "string",
          "const": "LFE"
        },
        {
          "description": "Arrière gauche.",
          "type": "string",
          "const": "RL"
        },
        {
          "description": "Arrière droit.",
          "type": "string",
          "const": "RR"
        },
        {
          "description": "Latéral gauche.",
          "type": "string",
          "const": "SL"
        },
        {
          "description": "Latéral droit.",
          "type": "string",
          "const": "SR"
        },
        {
          "description": "Auxiliaire numéroté (sans position spatiale).",
          "type": "string",
          "const": "AUX"
        }
      ]
    },
    "Command": {
      "description": "Commande adressée au moteur.",
      "oneOf": [
        {
          "description": "État global.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "status"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Liste des nœuds.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "nodes"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Description d'un nœud et de ses ports.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "ports"
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            }
          },
          "required": [
            "cmd",
            "node"
          ]
        },
        {
          "description": "Liste des liens.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "links"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Crée un lien.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "link"
            },
            "dst": {
              "description": "Destination (entrée).",
              "$ref": "#/$defs/PortId"
            },
            "src": {
              "description": "Source (sortie).",
              "$ref": "#/$defs/PortId"
            }
          },
          "required": [
            "cmd",
            "src",
            "dst"
          ]
        },
        {
          "description": "Crée un lien par noms de ports.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "link_by_name"
            },
            "dst_node": {
              "description": "Nœud destination.",
              "$ref": "#/$defs/NodeId"
            },
            "dst_port": {
              "description": "Port d'entrée.",
              "type": "string"
            },
            "src_node": {
              "description": "Nœud source.",
              "$ref": "#/$defs/NodeId"
            },
            "src_port": {
              "description": "Port de sortie.",
              "type": "string"
            }
          },
          "required": [
            "cmd",
            "src_node",
            "src_port",
            "dst_node",
            "dst_port"
          ]
        },
        {
          "description": "Supprime un lien.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "unlink"
            },
            "link": {
              "description": "Lien.",
              "$ref": "#/$defs/LinkId"
            }
          },
          "required": [
            "cmd",
            "link"
          ]
        },
        {
          "description": "Gain d'un nœud.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "set_node_gain"
            },
            "gain_db": {
              "description": "Gain (dB), `None` = inchangé.",
              "anyOf": [
                {
                  "$ref": "#/$defs/Db"
                },
                {
                  "type": "null"
                }
              ]
            },
            "muted": {
              "description": "Muet, `None` = inchangé.",
              "type": [
                "boolean",
                "null"
              ]
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            }
          },
          "required": [
            "cmd",
            "node"
          ]
        },
        {
          "description": "Gain d'un lien.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "set_link_gain"
            },
            "gain_db": {
              "description": "Gain (dB).",
              "anyOf": [
                {
                  "$ref": "#/$defs/Db"
                },
                {
                  "type": "null"
                }
              ]
            },
            "link": {
              "description": "Lien.",
              "$ref": "#/$defs/LinkId"
            },
            "muted": {
              "description": "Muet.",
              "type": [
                "boolean",
                "null"
              ]
            }
          },
          "required": [
            "cmd",
            "link"
          ]
        },
        {
          "description": "Nom d'affichage d'un nœud.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "set_label"
            },
            "label": {
              "description": "Nom.",
              "type": "string"
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            }
          },
          "required": [
            "cmd",
            "node",
            "label"
          ]
        },
        {
          "description": "Change le pilote.",
          "type": "object",
          "properties": {
            "choice": {
              "description": "Choix.",
              "$ref": "#/$defs/DriverChoice"
            },
            "cmd": {
              "type": "string",
              "const": "set_driver"
            }
          },
          "required": [
            "cmd",
            "choice"
          ]
        },
        {
          "description": "Ajoute un nœud interne.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "add_internal"
            },
            "kind": {
              "description": "Type.",
              "$ref": "#/$defs/InternalKind"
            },
            "name": {
              "description": "Nom unique.",
              "type": "string"
            }
          },
          "required": [
            "cmd",
            "name",
            "kind"
          ]
        },
        {
          "description": "Retire un nœud (interne, ou périphérique absent).",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "remove_node"
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            }
          },
          "required": [
            "cmd",
            "node"
          ]
        },
        {
          "description": "Règle un paramètre d'un nœud interne (`frequency`, `amplitude`,\n`band.<i>.{frequency,q,gain_db,enabled}`).",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "set_param"
            },
            "name": {
              "description": "Nom.",
              "type": "string"
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            },
            "value": {
              "description": "Valeur.",
              "type": "number",
              "format": "float"
            }
          },
          "required": [
            "cmd",
            "node",
            "name",
            "value"
          ]
        },
        {
          "description": "Lit un VU-mètre.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "read_meter"
            },
            "node": {
              "description": "Nœud.",
              "$ref": "#/$defs/NodeId"
            }
          },
          "required": [
            "cmd",
            "node"
          ]
        },
        {
          "description": "Liste les câbles.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "cable_list"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Crée un câble.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "cable_add"
            },
            "spec": {
              "description": "Spécification.",
              "$ref": "#/$defs/CableSpec"
            }
          },
          "required": [
            "cmd",
            "spec"
          ]
        },
        {
          "description": "Supprime un câble.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "cable_remove"
            },
            "id": {
              "description": "Câble.",
              "$ref": "#/$defs/CableId"
            }
          },
          "required": [
            "cmd",
            "id"
          ]
        },
        {
          "description": "Renomme un câble.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "cable_rename"
            },
            "id": {
              "description": "Câble.",
              "$ref": "#/$defs/CableId"
            },
            "name": {
              "description": "Nom.",
              "type": "string"
            }
          },
          "required": [
            "cmd",
            "id",
            "name"
          ]
        },
        {
          "description": "Change les canaux d'un câble.",
          "type": "object",
          "properties": {
            "channels": {
              "description": "Canaux.",
              "$ref": "#/$defs/ChannelCount"
            },
            "cmd": {
              "type": "string",
              "const": "cable_set_channels"
            },
            "id": {
              "description": "Câble.",
              "$ref": "#/$defs/CableId"
            }
          },
          "required": [
            "cmd",
            "id",
            "channels"
          ]
        },
        {
          "description": "Change le **format** d'un câble : fréquence, profondeur, canaux.\n\nSous Windows, la seule commande qui change réellement les canaux d'un câble —\n`cable_set_channels` se fait refuser par le pilote dès que le compte demandé n'est\npas celui du format configuré. Elle exige un câble **déconnecté** et coûte environ\nune seconde de silence sur les seize câbles : le format d'un endpoint est figé à sa\ncréation, et l'appliquer demande de redémarrer le périphérique du pilote.\n\nAjoutée après [`PROTOCOL_VERSION`](crate::wire::PROTOCOL_VERSION) 1 sans\nl'incrémenter : c'est une **variante de plus** dans un énuméré étiqueté par `cmd`,\nqu'un client plus ancien n'émet jamais et dont il n'a donc rien à savoir.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "cable_set_format"
            },
            "format": {
              "description": "Format voulu.",
              "$ref": "#/$defs/CableFormat"
            },
            "id": {
              "description": "Câble.",
              "$ref": "#/$defs/CableId"
            }
          },
          "required": [
            "cmd",
            "id",
            "format"
          ]
        },
        {
          "description": "Remet les compteurs de xruns à zéro.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "reset_xruns"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "S'abonne (ou se désabonne) aux notifications.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "subscribe"
            },
            "enabled": {
              "description": "Recevoir les notifications.",
              "type": "boolean"
            }
          },
          "required": [
            "cmd",
            "enabled"
          ]
        },
        {
          "description": "Rapport de diagnostic (sans donnée personnelle).",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "dump"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Sauvegarde l'état maintenant.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "save"
            }
          },
          "required": [
            "cmd"
          ]
        },
        {
          "description": "Recharge l'état persisté.",
          "type": "object",
          "properties": {
            "cmd": {
              "type": "string",
              "const": "load"
            }
          },
          "required": [
            "cmd"
          ]
        }
      ]
    },
    "Db": {
      "oneOf": [
        {
          "description": "Gain en dB",
          "type": "number",
          "maximum": 24.0
        },
        {
          "description": "Silence",
          "type": "string",
          "enum": [
            "-inf"
          ]
        }
      ]
    },
    "DeviceDirection": {
      "description": "Sens d'un périphérique, vu de l'application.",
      "oneOf": [
        {
          "description": "Le périphérique fournit de l'audio (micro, entrée de câble).",
          "type": "string",
          "const": "capture"
        },
        {
          "description": "Le périphérique consomme de l'audio (haut-parleurs, sortie de câble).",
          "type": "string",
          "const": "render"
        }
      ]
    },
    "DeviceId": {
      "description": "Identifiant stable d'un périphérique, fourni par l'OS (identifiant d'endpoint\nWASAPI, nom de nœud PipeWire, UID CoreAudio). Ne dépend pas de l'ordre\nd'énumération.",
      "type": "string"
    },
    "DeviceInfo": {
      "description": "Description d'un périphérique énuméré.",
      "type": "object",
      "properties": {
        "cable": {
          "description": "Identifiant du câble Conduit si ce périphérique en est un côté.",
          "anyOf": [
            {
              "$ref": "#/$defs/CableId"
            },
            {
              "type": "null"
            }
          ]
        },
        "channels": {
          "description": "Nombre de canaux natif.",
          "type": "integer",
          "format": "uint",
          "minimum": 0
        },
        "default_block": {
          "description": "Taille de bloc par défaut (trames par rappel).",
          "type": "integer",
          "format": "uint",
          "minimum": 0
        },
        "direction": {
          "description": "Sens.",
          "$ref": "#/$defs/DeviceDirection"
        },
        "id": {
          "description": "Identifiant stable.",
          "$ref": "#/$defs/DeviceId"
        },
        "is_default": {
          "description": "Vrai si c'est le périphérique par défaut de l'OS pour son sens.",
          "type": "boolean"
        },
        "name": {
          "description": "Nom lisible.",
          "type": "string"
        },
        "sample_rate": {
          "description": "Fréquence native (celle utilisée si le format demandé n'est pas disponible).",
          "$ref": "#/$defs/SampleRate"
        },
        "sample_rates": {
          "description": "Fréquences acceptées (vide = seulement `sample_rate`).",
          "type": "array",
          "items": {
            "$ref": "#/$defs/SampleRate"
          }
        }
      },
      "required": [
        "id",
        "name",
        "direction",
        "channels",
        "sample_rate",
        "sample_rates",
        "default_block",
        "is_default"
      ]
    },
    "DeviceStatus": {
      "description": "État d'un périphérique.",
      "type": "object",
      "properties": {
        "fill": {
          "description": "Remplissage du tampon (trames périphérique).",
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        },
        "fill_max": {
          "description": "Remplissage maximal depuis la dernière remise à zéro des xruns.",
          "type": "integer",
          "format": "uint32",
          "default": 0,
          "minimum": 0
        },
        "fill_min": {
          "description": "Remplissage minimal depuis la dernière remise à zéro des xruns.",
          "type": "integer",
          "format": "uint32",
          "default": 0,
          "minimum": 0
        },
        "id": {
          "description": "Périphérique.",
          "$ref": "#/$defs/DeviceId"
        },
        "locked": {
          "description": "DLL verrouillée.",
          "type": "boolean"
        },
        "node": {
          "description": "Nœud.",
          "$ref": "#/$defs/NodeId"
        },
        "overruns": {
          "description": "Débordements.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "ratio": {
          "description": "Ratio de rééchantillonnage.",
          "type": "number",
          "format": "double"
        },
        "ratio_max_millionths": {
          "description": "Ratio maximal depuis la dernière remise à zéro des xruns, en millionièmes.",
          "type": "integer",
          "format": "uint64",
          "default": 0,
          "minimum": 0
        },
        "ratio_min_millionths": {
          "description": "Ratio minimal depuis la dernière remise à zéro des xruns, en millionièmes\n(1_000_000 = 1,0).",
          "type": "integer",
          "format": "uint64",
          "default": 0,
          "minimum": 0
        },
        "state": {
          "description": "État.",
          "$ref": "#/$defs/NodeState"
        },
        "underruns": {
          "description": "Sous-alimentations.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        }
      },
      "required": [
        "id",
        "node",
        "state",
        "underruns",
        "overruns",
        "fill",
        "ratio",
        "locked"
      ]
    },
    "Direction": {
      "description": "Direction d'un port, vue du nœud.",
      "oneOf": [
        {
          "description": "Le port reçoit de l'audio.",
          "type": "string",
          "const": "input"
        },
        {
          "description": "Le port produit de l'audio.",
          "type": "string",
          "const": "output"
        }
      ]
    },
    "DriverChoice": {
      "description": "Choix du pilote de graphe.",
      "oneOf": [
        {
          "description": "Périphérique de rendu par défaut, sinon capture par défaut, sinon horloge interne.",
          "type": "object",
          "properties": {
            "mode": {
              "type": "string",
              "const": "auto"
            }
          },
          "required": [
            "mode"
          ]
        },
        {
          "description": "Horloge interne.",
          "type": "object",
          "properties": {
            "mode": {
              "type": "string",
              "const": "internal"
            }
          },
          "required": [
            "mode"
          ]
        },
        {
          "description": "Un périphérique précis.",
          "type": "object",
          "properties": {
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/DeviceId"
            },
            "mode": {
              "type": "string",
              "const": "device"
            }
          },
          "required": [
            "mode",
            "id"
          ]
        }
      ]
    },
    "DriverStatus": {
      "description": "Pilote courant.",
      "oneOf": [
        {
          "description": "Aucun (moteur arrêté).",
          "type": "object",
          "properties": {
            "kind": {
              "type": "string",
              "const": "none"
            }
          },
          "required": [
            "kind"
          ]
        },
        {
          "description": "Horloge interne.",
          "type": "object",
          "properties": {
            "kind": {
              "type": "string",
              "const": "internal"
            }
          },
          "required": [
            "kind"
          ]
        },
        {
          "description": "Périphérique.",
          "type": "object",
          "properties": {
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/DeviceId"
            },
            "kind": {
              "type": "string",
              "const": "device"
            }
          },
          "required": [
            "kind",
            "id"
          ]
        }
      ]
    },
    "EngineEvent": {
      "description": "Événement du fil audio.",
      "oneOf": [
        {
          "description": "Le cycle a dépassé son budget.",
          "type": "object",
          "properties": {
            "cycle": {
              "description": "Numéro du cycle.",
              "type": "integer",
              "format": "uint64",
              "minimum": 0
            },
            "duration_ns": {
              "description": "Durée mesurée (ns).",
              "type": "integer",
              "format": "uint64",
              "minimum": 0
            },
            "type": {
              "type": "string",
              "const": "cycle_overrun"
            }
          },
          "required": [
            "type",
            "cycle",
            "duration_ns"
          ]
        },
        {
          "description": "Xrun sur un périphérique asynchrone.",
          "type": "object",
          "properties": {
            "device": {
              "description": "Périphérique.",
              "$ref": "#/$defs/DeviceId"
            },
            "type": {
              "type": "string",
              "const": "device_xrun"
            },
            "underrun": {
              "description": "Sous-alimentation (`true`) ou débordement.",
              "type": "boolean"
            }
          },
          "required": [
            "type",
            "device",
            "underrun"
          ]
        },
        {
          "description": "Le pilote de graphe a exécuté son premier cycle.",
          "type": "object",
          "properties": {
            "device": {
              "description": "Périphérique pilote (`None` = horloge interne).",
              "anyOf": [
                {
                  "$ref": "#/$defs/DeviceId"
                },
                {
                  "type": "null"
                }
              ]
            },
            "type": {
              "type": "string",
              "const": "driver_started"
            }
          },
          "required": [
            "type"
          ]
        },
        {
          "description": "L'exécuteur était occupé : cycle sauté, silence.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "executor_busy"
            }
          },
          "required": [
            "type"
          ]
        }
      ]
    },
    "EngineStatus": {
      "description": "État global du moteur.",
      "type": "object",
      "properties": {
        "backend": {
          "description": "Backend.",
          "type": "string"
        },
        "cycles": {
          "description": "Cycles exécutés.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "devices": {
          "description": "Périphériques.",
          "type": "array",
          "items": {
            "$ref": "#/$defs/DeviceStatus"
          }
        },
        "driver": {
          "description": "Pilote effectif.",
          "$ref": "#/$defs/DriverStatus"
        },
        "driver_choice": {
          "description": "Choix configuré.",
          "$ref": "#/$defs/DriverChoice"
        },
        "links": {
          "description": "Liens.",
          "type": "integer",
          "format": "uint",
          "minimum": 0
        },
        "nodes": {
          "description": "Nœuds.",
          "type": "integer",
          "format": "uint",
          "minimum": 0
        },
        "position": {
          "description": "Position du graphe (trames).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "quantum": {
          "description": "Quantum.",
          "$ref": "#/$defs/Quantum"
        },
        "sample_rate": {
          "description": "Fréquence.",
          "$ref": "#/$defs/SampleRate"
        },
        "timing": {
          "description": "Temps de cycle.",
          "$ref": "#/$defs/TimingSnapshot"
        },
        "xruns": {
          "description": "Xruns cumulés.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        }
      },
      "required": [
        "backend",
        "sample_rate",
        "quantum",
        "driver",
        "driver_choice",
        "nodes",
        "links",
        "timing",
        "xruns",
        "devices",
        "position",
        "cycles"
      ]
    },
    "EqBand": {
      "description": "Réglage d'une bande.",
      "type": "object",
      "properties": {
        "enabled": {
          "description": "Bande active.",
          "type": "boolean"
        },
        "frequency": {
          "description": "Fréquence centrale ou de coupure, en Hz.",
          "type": "number",
          "format": "float"
        },
        "gain_db": {
          "description": "Gain en dB (cloche et plateaux).",
          "type": "number",
          "format": "float"
        },
        "kind": {
          "description": "Type de filtre.",
          "$ref": "#/$defs/FilterKind"
        },
        "q": {
          "description": "Facteur de qualité.",
          "type": "number",
          "format": "float"
        }
      },
      "required": [
        "kind",
        "frequency",
        "q",
        "gain_db",
        "enabled"
      ]
    },
    "ErrorCode": {
      "description": "Code d'erreur stable, pour les clients.",
      "oneOf": [
        {
          "description": "Nœud, port ou lien inconnu.",
          "type": "string",
          "const": "not_found"
        },
        {
          "description": "Le lien créerait une boucle.",
          "type": "string",
          "const": "would_cycle"
        },
        {
          "description": "Requête invalide (direction, doublon, paramètre).",
          "type": "string",
          "const": "invalid"
        },
        {
          "description": "Périphérique ou backend en erreur.",
          "type": "string",
          "const": "device"
        },
        {
          "description": "Câble : limite, droits, non supporté.",
          "type": "string",
          "const": "cable"
        },
        {
          "description": "Le moteur ne peut pas appliquer maintenant (réessayer).",
          "type": "string",
          "const": "busy"
        },
        {
          "description": "Commande inconnue ou non supportée par ce démon.",
          "type": "string",
          "const": "unsupported"
        },
        {
          "description": "Erreur interne.",
          "type": "string",
          "const": "internal"
        }
      ]
    },
    "Event": {
      "description": "Événement diffusé aux abonnés.",
      "type": "object",
      "properties": {
        "notification": {
          "description": "Notification.",
          "$ref": "#/$defs/Notification"
        }
      },
      "required": [
        "notification"
      ]
    },
    "FilterKind": {
      "description": "Type de filtre.",
      "oneOf": [
        {
          "description": "Passe-bas (pente 12 dB/oct).",
          "type": "string",
          "const": "low_pass"
        },
        {
          "description": "Passe-haut.",
          "type": "string",
          "const": "high_pass"
        },
        {
          "description": "Passe-bande (gain 0 dB au centre).",
          "type": "string",
          "const": "band_pass"
        },
        {
          "description": "Coupe-bande.",
          "type": "string",
          "const": "notch"
        },
        {
          "description": "Passe-tout (phase seulement).",
          "type": "string",
          "const": "all_pass"
        },
        {
          "description": "Cloche (égaliseur paramétrique).",
          "type": "string",
          "const": "peaking"
        },
        {
          "description": "Plateau grave.",
          "type": "string",
          "const": "low_shelf"
        },
        {
          "description": "Plateau aigu.",
          "type": "string",
          "const": "high_shelf"
        }
      ]
    },
    "Hello": {
      "description": "Premier message envoyé par un client.",
      "type": "object",
      "properties": {
        "client": {
          "description": "Nom du client (`\"conduitctl 0.1.0\"`), pour les journaux.",
          "type": "string"
        },
        "version": {
          "description": "Version du protocole parlée par le client.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "version",
        "client"
      ]
    },
    "HelloReply": {
      "description": "Réponse du démon à [`Hello`].",
      "type": "object",
      "properties": {
        "accepted": {
          "description": "Vrai si le démon accepte de continuer avec ce client.",
          "type": "boolean"
        },
        "reason": {
          "description": "Explication en cas de refus (quoi faire).",
          "type": [
            "string",
            "null"
          ]
        },
        "server": {
          "description": "Nom et version du démon.",
          "type": "string"
        },
        "version": {
          "description": "Version du protocole du démon.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "version",
        "server",
        "accepted"
      ]
    },
    "InternalKind": {
      "description": "Type de nœud interne à créer.",
      "oneOf": [
        {
          "description": "Silence.",
          "type": "object",
          "properties": {
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "silence"
            }
          },
          "required": [
            "kind",
            "channels"
          ]
        },
        {
          "description": "Sinus.",
          "type": "object",
          "properties": {
            "amplitude": {
              "description": "Amplitude (crête).",
              "type": "number",
              "format": "float"
            },
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "frequency": {
              "description": "Fréquence (Hz).",
              "type": "number",
              "format": "float"
            },
            "kind": {
              "type": "string",
              "const": "sine"
            }
          },
          "required": [
            "kind",
            "frequency",
            "amplitude",
            "channels"
          ]
        },
        {
          "description": "Bruit.",
          "type": "object",
          "properties": {
            "amplitude": {
              "description": "Amplitude.",
              "type": "number",
              "format": "float"
            },
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "noise"
            },
            "pink": {
              "description": "Rose (`true`) ou blanc.",
              "type": "boolean"
            }
          },
          "required": [
            "kind",
            "pink",
            "amplitude",
            "channels"
          ]
        },
        {
          "description": "Mixeur N bus → 1.",
          "type": "object",
          "properties": {
            "buses": {
              "description": "Bus.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "channels": {
              "description": "Canaux par bus.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "mixer"
            }
          },
          "required": [
            "kind",
            "buses",
            "channels"
          ]
        },
        {
          "description": "Duplicateur 1 → N.",
          "type": "object",
          "properties": {
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "copies": {
              "description": "Copies.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "splitter"
            }
          },
          "required": [
            "kind",
            "channels",
            "copies"
          ]
        },
        {
          "description": "VU-mètre.",
          "type": "object",
          "properties": {
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "meter"
            }
          },
          "required": [
            "kind",
            "channels"
          ]
        },
        {
          "description": "Égaliseur paramétrique.",
          "type": "object",
          "properties": {
            "bands": {
              "description": "Bandes.",
              "type": "array",
              "items": {
                "$ref": "#/$defs/EqBand"
              }
            },
            "channels": {
              "description": "Canaux.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "equalizer"
            }
          },
          "required": [
            "kind",
            "channels",
            "bands"
          ]
        },
        {
          "description": "Adaptation de canaux automatique.",
          "type": "object",
          "properties": {
            "inputs": {
              "description": "Entrées.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            },
            "kind": {
              "type": "string",
              "const": "adapter"
            },
            "outputs": {
              "description": "Sorties.",
              "type": "integer",
              "format": "uint",
              "minimum": 0
            }
          },
          "required": [
            "kind",
            "inputs",
            "outputs"
          ]
        }
      ]
    },
    "LinkDescriptor": {
      "description": "Description d'un lien.",
      "type": "object",
      "properties": {
        "dst": {
          "description": "Port destination (une entrée).",
          "$ref": "#/$defs/PortId"
        },
        "gain_db": {
          "description": "Gain (dB).",
          "$ref": "#/$defs/Db"
        },
        "id": {
          "description": "Identifiant.",
          "$ref": "#/$defs/LinkId"
        },
        "muted": {
          "description": "Muet.",
          "type": "boolean"
        },
        "src": {
          "description": "Port source (une sortie).",
          "$ref": "#/$defs/PortId"
        }
      },
      "required": [
        "id",
        "src",
        "dst",
        "gain_db",
        "muted"
      ]
    },
    "LinkId": {
      "description": "Identifiant d'un lien.",
      "type": "object",
      "properties": {
        "generation": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        },
        "index": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "index",
        "generation"
      ]
    },
    "MeterReading": {
      "description": "Mesure d'un canal.",
      "type": "object",
      "properties": {
        "peak": {
          "description": "Crête avec décroissance (linéaire, ≥ 0).",
          "type": "number",
          "format": "float"
        },
        "peak_hold": {
          "description": "Crête maximale depuis la dernière remise à zéro.",
          "type": "number",
          "format": "float"
        },
        "rms": {
          "description": "RMS lissé (linéaire, ≥ 0).",
          "type": "number",
          "format": "float"
        }
      },
      "required": [
        "peak",
        "rms",
        "peak_hold"
      ]
    },
    "NodeDescriptor": {
      "description": "Description d'un nœud.",
      "type": "object",
      "properties": {
        "device": {
          "description": "Périphérique associé.",
          "anyOf": [
            {
              "$ref": "#/$defs/DeviceInfo"
            },
            {
              "type": "null"
            }
          ]
        },
        "gain_db": {
          "description": "Gain (dB).",
          "$ref": "#/$defs/Db"
        },
        "id": {
          "description": "Identifiant de graphe (peut changer si le nœud est recréé).",
          "$ref": "#/$defs/NodeId"
        },
        "inputs": {
          "description": "Entrées.",
          "type": "array",
          "items": {
            "$ref": "#/$defs/PortSpec"
          }
        },
        "key": {
          "description": "Clé stable.",
          "$ref": "#/$defs/NodeKey"
        },
        "label": {
          "description": "Nom d'affichage.",
          "type": "string"
        },
        "muted": {
          "description": "Muet.",
          "type": "boolean"
        },
        "outputs": {
          "description": "Sorties.",
          "type": "array",
          "items": {
            "$ref": "#/$defs/PortSpec"
          }
        },
        "state": {
          "description": "État.",
          "$ref": "#/$defs/NodeState"
        },
        "type_name": {
          "description": "Type (`\"sine\"`, `\"device-render\"`, …).",
          "type": "string"
        }
      },
      "required": [
        "id",
        "key",
        "label",
        "type_name",
        "inputs",
        "outputs",
        "state",
        "gain_db",
        "muted"
      ]
    },
    "NodeId": {
      "description": "Identifiant d'un nœud. Un identifiant n'est jamais réutilisé après suppression.",
      "type": "object",
      "properties": {
        "generation": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        },
        "index": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "index",
        "generation"
      ]
    },
    "NodeKey": {
      "description": "Clé stable d'un nœud (miroir de `conduit_engine::NodeKey`).",
      "oneOf": [
        {
          "description": "Périphérique d'un backend.",
          "type": "object",
          "properties": {
            "backend": {
              "description": "Nom du backend.",
              "type": "string"
            },
            "id": {
              "description": "Identifiant OS.",
              "$ref": "#/$defs/DeviceId"
            },
            "kind": {
              "type": "string",
              "const": "device"
            }
          },
          "required": [
            "kind",
            "backend",
            "id"
          ]
        },
        {
          "description": "Nœud interne nommé.",
          "type": "object",
          "properties": {
            "kind": {
              "type": "string",
              "const": "internal"
            },
            "name": {
              "description": "Nom unique.",
              "type": "string"
            }
          },
          "required": [
            "kind",
            "name"
          ]
        }
      ]
    },
    "NodeState": {
      "description": "État d'un nœud dans le moteur.",
      "oneOf": [
        {
          "description": "Nœud interne, toujours actif.",
          "type": "string",
          "const": "internal"
        },
        {
          "description": "Périphérique ouvert en asynchrone.",
          "type": "string",
          "const": "active"
        },
        {
          "description": "Périphérique pilote du graphe.",
          "type": "string",
          "const": "driver"
        },
        {
          "description": "Périphérique absent ou fermé ; liens conservés.",
          "type": "string",
          "const": "suspended"
        }
      ]
    },
    "Notification": {
      "description": "Notification diffusée aux abonnés (F-43).",
      "oneOf": [
        {
          "description": "Nœud ajouté.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "node_added"
            }
          },
          "$ref": "#/$defs/NodeDescriptor",
          "required": [
            "event"
          ]
        },
        {
          "description": "Nœud retiré.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "node_removed"
            },
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/NodeId"
            },
            "key": {
              "description": "Clé.",
              "$ref": "#/$defs/NodeKey"
            }
          },
          "required": [
            "event",
            "id",
            "key"
          ]
        },
        {
          "description": "État d'un nœud changé.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "node_state_changed"
            },
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/NodeId"
            },
            "state": {
              "description": "Nouvel état.",
              "$ref": "#/$defs/NodeState"
            }
          },
          "required": [
            "event",
            "id",
            "state"
          ]
        },
        {
          "description": "Lien ajouté.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "link_added"
            }
          },
          "$ref": "#/$defs/LinkDescriptor",
          "required": [
            "event"
          ]
        },
        {
          "description": "Lien retiré.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "link_removed"
            },
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/LinkId"
            }
          },
          "required": [
            "event",
            "id"
          ]
        },
        {
          "description": "Pilote changé.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "driver_changed"
            }
          },
          "$ref": "#/$defs/DriverStatus",
          "required": [
            "event"
          ]
        },
        {
          "description": "Câble changé (`None` = supprimé).",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "cable_changed"
            },
            "id": {
              "description": "Identifiant.",
              "$ref": "#/$defs/CableId"
            },
            "info": {
              "description": "État.",
              "anyOf": [
                {
                  "$ref": "#/$defs/CableInfo"
                },
                {
                  "type": "null"
                }
              ]
            }
          },
          "required": [
            "event",
            "id"
          ]
        },
        {
          "description": "Événement du fil audio.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "rt"
            }
          },
          "$ref": "#/$defs/EngineEvent",
          "required": [
            "event"
          ]
        },
        {
          "description": "Le démon s'arrête.",
          "type": "object",
          "properties": {
            "event": {
              "type": "string",
              "const": "shutdown"
            }
          },
          "required": [
            "event"
          ]
        }
      ]
    },
    "PortId": {
      "description": "Identifiant d'un port : nœud, direction et index dans cette direction.",
      "type": "object",
      "properties": {
        "direction": {
          "description": "Direction du port.",
          "$ref": "#/$defs/Direction"
        },
        "index": {
          "description": "Index dans les ports de cette direction.",
          "type": "integer",
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "node": {
          "description": "Nœud propriétaire.",
          "$ref": "#/$defs/NodeId"
        }
      },
      "required": [
        "node",
        "direction",
        "index"
      ]
    },
    "PortSpec": {
      "description": "Description d'un port mono d'un nœud.",
      "type": "object",
      "properties": {
        "label": {
          "description": "Position de canal.",
          "$ref": "#/$defs/ChannelLabel"
        },
        "name": {
          "description": "Nom du port, unique parmi les ports de même direction d'un nœud.",
          "type": "string"
        }
      },
      "required": [
        "name",
        "label"
      ]
    },
    "ProtocolError": {
      "description": "Erreur renvoyée à un client : code stable + message destiné à l'utilisateur.",
      "type": "object",
      "properties": {
        "code": {
          "description": "Code.",
          "$ref": "#/$defs/ErrorCode"
        },
        "message": {
          "description": "Message lisible, indiquant quoi faire.",
          "type": "string"
        }
      },
      "required": [
        "code",
        "message"
      ]
    },
    "Quantum": {
      "description": "Trames par cycle, puissance de deux",
      "type": "integer",
      "maximum": 8192,
      "minimum": 32
    },
    "Reply": {
      "description": "Réponse à une commande.",
      "oneOf": [
        {
          "description": "Succès sans donnée.",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "ok"
            }
          },
          "required": [
            "reply"
          ]
        },
        {
          "description": "État.",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "status"
            }
          },
          "$ref": "#/$defs/EngineStatus",
          "required": [
            "reply"
          ]
        },
        {
          "description": "Nœuds.",
          "type": "object",
          "properties": {
            "nodes": {
              "description": "Liste.",
              "type": "array",
              "items": {
                "$ref": "#/$defs/NodeDescriptor"
              }
            },
            "reply": {
              "type": "string",
              "const": "nodes"
            }
          },
          "required": [
            "reply",
            "nodes"
          ]
        },
        {
          "description": "Un nœud.",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "node"
            }
          },
          "$ref": "#/$defs/NodeDescriptor",
          "required": [
            "reply"
          ]
        },
        {
          "description": "Liens.",
          "type": "object",
          "properties": {
            "links": {
              "description": "Liste.",
              "type": "array",
              "items": {
                "$ref": "#/$defs/LinkDescriptor"
              }
            },
            "reply": {
              "type": "string",
              "const": "links"
            }
          },
          "required": [
            "reply",
            "links"
          ]
        },
        {
          "description": "Un lien.",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "link"
            }
          },
          "$ref": "#/$defs/LinkDescriptor",
          "required": [
            "reply"
          ]
        },
        {
          "description": "Mesures d'un VU-mètre, un élément par canal.",
          "type": "object",
          "properties": {
            "channels": {
              "description": "Canaux.",
              "type": "array",
              "items": {
                "$ref": "#/$defs/MeterReading"
              }
            },
            "reply": {
              "type": "string",
              "const": "meter"
            }
          },
          "required": [
            "reply",
            "channels"
          ]
        },
        {
          "description": "Câbles.",
          "type": "object",
          "properties": {
            "cables": {
              "description": "Liste.",
              "type": "array",
              "items": {
                "$ref": "#/$defs/CableInfo"
              }
            },
            "reply": {
              "type": "string",
              "const": "cables"
            }
          },
          "required": [
            "reply",
            "cables"
          ]
        },
        {
          "description": "Un câble.",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "cable"
            }
          },
          "$ref": "#/$defs/CableInfo",
          "required": [
            "reply"
          ]
        },
        {
          "description": "Rapport de diagnostic (texte).",
          "type": "object",
          "properties": {
            "reply": {
              "type": "string",
              "const": "dump"
            },
            "text": {
              "description": "Texte.",
              "type": "string"
            }
          },
          "required": [
            "reply",
            "text"
          ]
        }
      ]
    },
    "Request": {
      "description": "Requête : identifiant + commande.",
      "type": "object",
      "properties": {
        "command": {
          "description": "Commande.",
          "$ref": "#/$defs/Command"
        },
        "id": {
          "description": "Identifiant choisi par le client.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "id",
        "command"
      ]
    },
    "Response": {
      "description": "Réponse : identifiant + résultat.",
      "type": "object",
      "properties": {
        "id": {
          "description": "Identifiant de la requête.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        },
        "result": {
          "description": "Résultat.",
          "$ref": "#/$defs/Result_of_Reply_or_ProtocolError"
        }
      },
      "required": [
        "id",
        "result"
      ]
    },
    "Result_of_Reply_or_ProtocolError": {
      "oneOf": [
        {
          "type": "object",
          "properties": {
            "Ok": {
              "$ref": "#/$defs/Reply"
            }
          },
          "required": [
            "Ok"
          ]
        },
        {
          "type": "object",
          "properties": {
            "Err": {
              "$ref": "#/$defs/ProtocolError"
            }
          },
          "required": [
            "Err"
          ]
        }
      ]
    },
    "SampleDepth": {
      "description": "Profondeur d'échantillon d'un câble.\n\n# Pourquoi un type d'ici et non `conduit_kmd_core::ring::SampleFormat`\n\nCe crate est la **couche partagée** des trois plateformes : il porte le trait\n[`CableControl`] que servent le dorsal WASAPI, le dorsal PipeWire et le dorsal\nCoreAudio, et il ne dépend pas — et ne doit pas dépendre — de `conduit-kmd-core`, qui\nest le contrat d'**un** pilote Windows. Un câble PipeWire a une profondeur sans que\n`CableFormat<n>` existe nulle part sur la machine.\n\nLa traduction vers l'encodage `REG_DWORD` du pilote vit donc là où les deux mondes se\ntouchent, et nulle part ailleurs : `conduit_helper::controle`.",
      "oneOf": [
        {
          "description": "PCM signé 16 bits.",
          "type": "string",
          "const": "pcm16"
        },
        {
          "description": "PCM signé 24 bits (conteneur de trois octets).",
          "type": "string",
          "const": "pcm24"
        },
        {
          "description": "Flottant 32 bits, plage nominale [−1, 1]. Le défaut du moteur audio de Windows.",
          "type": "string",
          "const": "f32"
        }
      ]
    },
    "SampleRate": {
      "description": "Fréquence d'échantillonnage en hertz.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0
    },
    "TimingSnapshot": {
      "description": "Instantané des temps de cycle.",
      "type": "object",
      "properties": {
        "avg_ns": {
          "description": "Durée moyenne (ns).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "budget_ns": {
          "description": "Budget d'un cycle (ns).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "count": {
          "description": "Cycles mesurés.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "last_ns": {
          "description": "Dernière durée (ns).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "max_ns": {
          "description": "Durée maximale (ns).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "min_ns": {
          "description": "Durée minimale (ns).",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "overruns": {
          "description": "Cycles au-delà du budget.",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        }
      },
      "required": [
        "count",
        "min_ns",
        "avg_ns",
        "max_ns",
        "last_ns",
        "budget_ns",
        "overruns"
      ]
    }
  }
}
```
