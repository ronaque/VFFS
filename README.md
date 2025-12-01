# VFFS

Este projeto é uma implementação de um sistema de arquivos virtual implementado com o uso da biblioteca Fuser, na 
qual permite a montagem de um sistema de arquivos no userspace.

## Requisitos

O projeto só pode ser rodado em um sistema operacional Linux, e requer a execução da seguinte instalação:
```bash
sudo apt-get install fuse3 libfuse3-dev
```

Para mais detalhes, visitar o repositório do projeto [fuser](https://github.com/cberner/fuser)

## Execução

Para executar o projeto, utilize o comando abaixo, substituindo `<MOUNT_POINT>` pelo diretório onde deseja montar o 
sistema de arquivos e `<MEMORY_LIMIT_IN_MB>` pelo limite de memória em megabytes que o sistema de arquivos pode utilizar.

```bash
cargo run -- --mount-point <MOUNT_POINT> --memory-limit <MEMORY_LIMIT_IN_MB>
```

Além destes parâmetros obrigatórios, existem outros parâmetros opcionais que podem ser utilizados:
- `-v`: Define o nível de log com a contagem de repetições do parâmetro
- `--max-file-size <SIZE_IN_MB>`: Define o tamanho máximo de um arquivo em megabytes. O padrão é 1 MB.