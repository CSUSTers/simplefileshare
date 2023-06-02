# sIMPLEfILEsHARE
简单的文件共享服务器 in rust。

主要功能：
 - 多用户管理，不同用户都可通过其token上传文件，后期也不一定会有更多玩法
 - 带基本的权限管理，需传入token才可下载文件，想要更多权限控制请自己写
 - 可设置文件过期时间，过期后别想下载

# 用于

[CSUSTers/csust-got](https://github.com/CSUSTers/csust-got)
[Anthony-Hoo/voiceGen](https://github.com/Anthony-Hoo/voiceGen)