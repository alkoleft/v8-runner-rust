## 7. Представление развёртывания

Основная цель развёртывания — одна рабочая станция разработчика или локальный automation-host с доступом к файловой системе и установленными утилитами 1С.

```mermaid
flowchart TB
    subgraph Host["Машина разработчика / локальный automation-host"]
        Binary["Бинарь v8-runner"]
        Config["v8project.yaml"]
        Sources["Исходники проекта"]
        Work["workPath\nлоги, temp, хеши, edt-workspace"]
        Binary --> Config
        Binary --> Sources
        Binary --> Work
        Binary --> Tools["Локальные утилиты 1С\n1cv8 / 1cv8c / ibcmd / 1cedtcli"]
        Binary --> HTTP["Опциональный MCP HTTP listener"]
    end

    Assistant["MCP-клиент / AI-ассистент"] --> HTTP
    Developer["Пользователь терминала"] --> Binary
    Tools --> Infobase["Локальная файловая ИБ"]
```

Предположения по развёртыванию:

- процесс может запускать дочерние процессы;
- настроенный `workPath` доступен на запись;
- деревья исходников и пути к ИБ доступны локально;
- отдельный database service самому `v8-runner` не нужен;
- HTTP listener нужен только для MCP transport и не участвует в обычном CLI path.
