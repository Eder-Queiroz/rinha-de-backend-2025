# Rinha de Backend 2025

## Tecnologias utilizadas

- **Linguagem:** Go
- **Armazenamento/Fila:** Redis
- **Balanceador:** Nginx
- **HTTP:** Fiber
- **Orquestração:** Docker Compose

## Como rodar

1. Suba o docker-compose dos Payment Processors primeiro (conforme instruções do repositório oficial).
2. Depois, suba este compose:
   ```sh
   docker compose up --build
   ```
3. O backend ficará disponível na porta **9999**.

## Sobre a solução

    Este projeto foi desenvolvido utilizando Go e o framework http `fiber`, foi utilizado `redis` como mecanismo de cache e único banco de dados, e também para processamento de Filas. Sobre a arquitetura do projeto ele é estruturado de forma em que as requisições chegam e são enviadas para uma fila a fim de serem processadas de acordo com as disponibilidades de Workers. Em paralelo um HealthCheckWorker verifica os status dos endpoints default e fallback, salva em cache a melhor possibilidade de uso no momento atual.


## Repositório do código-fonte

[https://github.com/JoaoReisA/rinha-de-backend-2025-go](https://github.com/JoaoReisA/rinha-de-backend-2025-go)