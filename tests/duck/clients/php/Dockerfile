# Two stages so that no `apt-get` runs at all: the composer image already has
# git, unzip and a PHP to run them with, and the runtime image only needs the
# resolved `vendor/` directory. Installing those through apt in the php image
# took longer than the rest of this suite put together.
FROM composer:2 AS vendor
WORKDIR /app
COPY composer.json ./
RUN composer install --no-interaction --no-progress --no-dev

FROM php:8.3-cli
WORKDIR /app
COPY --from=vendor /app/vendor ./vendor
COPY duck.php ./
CMD ["php", "duck.php"]
